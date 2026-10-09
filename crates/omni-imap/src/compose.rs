//! Durable compose receipts and workflows behind `email_draft_create`,
//! `email_send`, `email_send_status` and `email_sent_copy_repair`.
//!
//! Drafts and sends reserve their idempotency key durably before any side
//! effect. Uncertain sends are never retried; a partial recipient rejection is
//! never success. Composed sends persist the exact MIME before SMTP, record
//! SMTP acceptance before the Sent APPEND, and keep Sent verification separate
//! from delivery. Raw keys: `email-compose:{draft|send}:<sha256hex(key)>`.
//!
//! Attachments are pinned references re-read server-side
//! ([`crate::compose_attachments`]). Known keys settle from their receipt
//! before any source is re-read, so retries never resend. Receipts drop MIME no
//! later step uses: `wire` after SMTP, the private copy once it is verified.

use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use futures::future::BoxFuture;
use omni_core::clock::SharedClock;
use omni_store::cbor::{self, Extra, JsValue};
use omni_store::{DocMeta, DocOps as _, DocWrite as _, Store, StoreError, Tx};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest as _, Sha256};

use crate::attachments::AttachmentReader;
use crate::compose_attachments::{
    OutgoingAttachmentMetadata, OutgoingAttachmentReference, resolve_outgoing_attachments,
};
use crate::ops::drafts::{EmailDraftInput, EmailDraftResult};
use crate::ops::sent::{BeforeAppend, SentCopyInput, SentCopyResult};
use crate::protocol::ImapError;

const LOG: &str = "Email";

pub const DRAFT_ENTITY: &str = "email-compose-draft";
pub const SEND_ENTITY: &str = "email-compose-send";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComposeKind {
    Draft,
    Send,
}

impl ComposeKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Send => "send",
        }
    }

    fn entity(self) -> &'static str {
        match self {
            Self::Draft => DRAFT_ENTITY,
            Self::Send => SEND_ENTITY,
        }
    }
}

/// The persisted original MIME (`PreparedMessage`), base64 `wire` (SMTP, no
/// Bcc) and `content` (private Sent copy). `wire` is dropped once SMTP has an
/// outcome and `content` once the Sent copy is verified.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparedMessage {
    pub from: String,
    /// ISO send date.
    pub date: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wire: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `retainedMime`: `{from, date}` plus the private copy when it is still needed.
fn retained_mime(prepared: PreparedMessage, keep_content: bool) -> PreparedMessage {
    PreparedMessage {
        from: prepared.from,
        date: prepared.date,
        wire: None,
        content: prepared.content.filter(|c| keep_content && !c.is_empty()),
        extra: Extra::new(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AttemptStatus {
    Pending,
    Succeeded,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SentCopyState {
    Pending,
    Uncertain,
    Verified,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttemptResult {
    pub sent: bool,
    pub message_id: String,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `StoredAttempt`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredAttempt {
    pub fingerprint: String,
    pub status: AttemptStatus,
    pub updated_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prepared: Option<PreparedMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_copy: Option<SentCopyState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<OutgoingAttachmentMetadata>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<AttemptResult>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// The tools' `sentCopy` output, including `legacy-unavailable`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SentCopyReport {
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "uncertain")]
    Uncertain,
    #[serde(rename = "verified")]
    Verified,
    #[serde(rename = "legacy-unavailable")]
    LegacyUnavailable,
}

impl From<SentCopyState> for SentCopyReport {
    fn from(state: SentCopyState) -> Self {
        match state {
            SentCopyState::Pending => Self::Pending,
            SentCopyState::Uncertain => Self::Uncertain,
            SentCopyState::Verified => Self::Verified,
        }
    }
}

/// A compose failure. Display is the TS `EmailComposeError.message`; the
/// source chain ends at the innermost cause, which MCP errors report.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct EmailComposeError {
    pub message: String,
    #[source]
    pub cause: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl EmailComposeError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            cause: None,
        }
    }

    fn caused(
        message: impl Into<String>,
        cause: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        Self {
            message: message.into(),
            cause: Some(cause.into()),
        }
    }
}

/// `failWith`: MCP error text reports only the innermost cause, so the
/// guidance and the cause's message are flattened into one message.
fn fail_with<E: std::fmt::Display>(message: String) -> impl FnOnce(E) -> EmailComposeError {
    move |cause| EmailComposeError::new(format!("{message}: {cause}"))
}

/// SMTP submission of prebuilt MIME. `true` only when every recipient was accepted.
pub trait ComposeSender: Send + Sync {
    fn send<'a>(&'a self, recipients: &'a [String], raw: &'a [u8]) -> BoxFuture<'a, bool>;
}

/// The mailbox side (implemented by the IMAP transport).
pub trait ComposeMailbox: Send + Sync {
    /// False until the transport has started (TS `emailControls.transport` unset).
    fn available(&self) -> bool {
        true
    }
    fn create_draft<'a>(
        &'a self,
        input: &'a EmailDraftInput,
        allow_append: bool,
    ) -> BoxFuture<'a, Result<EmailDraftResult, ImapError>>;
    fn save_sent_copy<'a>(
        &'a self,
        input: &'a SentCopyInput,
        allow_append: bool,
        before_append: Option<BeforeAppend>,
    ) -> BoxFuture<'a, Result<SentCopyResult, ImapError>>;
}

/// `omni_mailer::Mailer` as a [`ComposeSender`] (`sendComposedEmailEffect`).
pub struct MailerSender {
    mailer: omni_mailer::Mailer,
}

impl MailerSender {
    pub fn new(mailer: omni_mailer::Mailer) -> Self {
        Self { mailer }
    }
}

impl ComposeSender for MailerSender {
    fn send<'a>(&'a self, recipients: &'a [String], raw: &'a [u8]) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            match self.mailer.send_raw(recipients, raw).await {
                Ok(report) => {
                    let all = recipients.iter().all(|r| report.accepted.contains(r));
                    let sent = report.rejected.is_empty() && all;
                    if !sent {
                        tracing::warn!(target: LOG, "SMTP rejected one or more composed email recipients");
                    }
                    sent
                }
                Err(error) => {
                    tracing::error!(target: LOG, "Failed to send composed email: {error}");
                    false
                }
            }
        })
    }
}

/// Normalized tool input (zod output: trimmed strings, `to` as a list).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ComposeRequest {
    pub idempotency_key: String,
    pub to: Vec<String>,
    pub cc: Option<Vec<String>>,
    pub bcc: Option<Vec<String>>,
    pub subject: String,
    pub text: String,
    pub in_reply_to: Option<String>,
    pub references: Option<Vec<String>>,
    /// `None` for an absent or empty list (the attachment-free fingerprint).
    pub attachments: Option<Vec<OutgoingAttachmentReference>>,
}

impl ComposeRequest {
    /// The zod-parsed object (schema key order, absent optionals omitted)
    /// plus `from`, as `JSON.stringify` sees it.
    fn fingerprint_json(&self, from: &str) -> String {
        let mut map = Map::new();
        map.insert(
            "idempotencyKey".to_owned(),
            Value::from(self.idempotency_key.clone()),
        );
        map.insert("to".to_owned(), Value::from(self.to.clone()));
        if let Some(cc) = &self.cc {
            map.insert("cc".to_owned(), Value::from(cc.clone()));
        }
        if let Some(bcc) = &self.bcc {
            map.insert("bcc".to_owned(), Value::from(bcc.clone()));
        }
        map.insert("subject".to_owned(), Value::from(self.subject.clone()));
        map.insert("text".to_owned(), Value::from(self.text.clone()));
        if let Some(in_reply_to) = &self.in_reply_to {
            map.insert("inReplyTo".to_owned(), Value::from(in_reply_to.clone()));
        }
        if let Some(references) = &self.references {
            map.insert("references".to_owned(), Value::from(references.clone()));
        }
        if let Some(attachments) = &self.attachments {
            let list = attachments
                .iter()
                .map(|r| {
                    let mut item = Map::new();
                    item.insert("messageId".to_owned(), Value::from(r.message_id.clone()));
                    item.insert(
                        "attachmentId".to_owned(),
                        Value::from(r.attachment_id.clone()),
                    );
                    item.insert("sha256".to_owned(), Value::from(r.sha256.clone()));
                    Value::Object(item)
                })
                .collect();
            map.insert("attachments".to_owned(), Value::Array(list));
        }
        map.insert("from".to_owned(), Value::from(from));
        omni_core::js::json_stringify(&Value::Object(map))
    }

    /// `sha256(JSON.stringify({...input, from}))`.
    pub fn fingerprint(&self, from: &str) -> String {
        hex::encode(Sha256::digest(self.fingerprint_json(from).as_bytes()))
    }

    fn draft_input(
        &self,
        attachments: Vec<omni_mailer::OutgoingEmailAttachment>,
    ) -> EmailDraftInput {
        EmailDraftInput {
            idempotency_key: self.idempotency_key.clone(),
            to: self.to.clone(),
            cc: self.cc.clone(),
            bcc: self.bcc.clone(),
            subject: self.subject.clone(),
            text: self.text.clone(),
            in_reply_to: self.in_reply_to.clone(),
            references: self.references.clone(),
            attachments,
        }
    }

    fn compose_input(
        &self,
        attachments: Vec<omni_mailer::OutgoingEmailAttachment>,
    ) -> omni_mailer::ComposeInput {
        omni_mailer::ComposeInput {
            to: self.to.clone(),
            cc: self.cc.clone().unwrap_or_default(),
            bcc: self.bcc.clone().unwrap_or_default(),
            subject: self.subject.clone(),
            text: self.text.clone(),
            in_reply_to: self.in_reply_to.clone(),
            references: self.references.clone().unwrap_or_default(),
            attachments,
        }
    }

    fn recipients(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for r in self
            .to
            .iter()
            .chain(self.cc.iter().flatten())
            .chain(self.bcc.iter().flatten())
        {
            if !out.contains(r) {
                out.push(r.clone());
            }
        }
        out
    }
}

pub fn key_for(kind: ComposeKind, idempotency_key: &str) -> String {
    format!(
        "email-compose:{}:{}",
        kind.as_str(),
        hex::encode(Sha256::digest(idempotency_key.as_bytes()))
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Reservation {
    Reserved,
    Succeeded {
        message_id: String,
        attachments: Vec<OutgoingAttachmentMetadata>,
    },
    Pending {
        attachments: Vec<OutgoingAttachmentMetadata>,
    },
    Failed,
    Mismatch,
}

/// `existingReservation`: how a stored receipt settles a repeated key.
fn existing_reservation(prior: StoredAttempt, fingerprint: &str) -> Reservation {
    if prior.fingerprint != fingerprint {
        return Reservation::Mismatch;
    }
    let attachments = prior.attachments.unwrap_or_default();
    match (prior.status, prior.result) {
        (AttemptStatus::Succeeded, Some(result)) if result.sent => Reservation::Succeeded {
            message_id: result.message_id,
            attachments,
        },
        (AttemptStatus::Pending, _) => Reservation::Pending { attachments },
        _ => Reservation::Failed,
    }
}

fn decode_attempt(pk: &str, value: JsValue) -> Result<StoredAttempt, StoreError> {
    cbor::from_value(value).map_err(|e| StoreError::CorruptRow {
        pk: pk.to_owned(),
        reason: e.to_string(),
    })
}

fn read_attempt_tx<D: omni_store::DocOps + ?Sized>(
    docs: &D,
    pk: &str,
) -> Result<Option<StoredAttempt>, StoreError> {
    match docs.get_raw_row(pk)? {
        Some(row) => decode_attempt(pk, row.decode()?).map(Some),
        None => Ok(None),
    }
}

fn write_attempt(
    tx: &mut Tx<'_>,
    pk: &str,
    kind: ComposeKind,
    attempt: &StoredAttempt,
) -> Result<(), StoreError> {
    let value = cbor::to_value(attempt).map_err(|source| StoreError::Encode {
        pk: pk.to_owned(),
        source,
    })?;
    tx.upsert_doc(
        pk,
        &value,
        DocMeta {
            entity: Some(kind.entity().to_owned()),
            ..DocMeta::default()
        },
    )
}

fn reservation_failure(action: &str, reservation: &Reservation) -> EmailComposeError {
    EmailComposeError::new(match reservation {
        Reservation::Mismatch => {
            format!("Idempotency key already belongs to different {action} content")
        }
        Reservation::Pending { .. } => format!(
            "A prior {action} attempt has an uncertain outcome; it will not be retried automatically. Do not retry with a new key without checking delivery."
        ),
        Reservation::Failed => format!(
            "A prior {action} attempt was not confirmed and will not be retried automatically. Do not retry with a new key without checking delivery."
        ),
        Reservation::Succeeded { .. } => {
            format!("Unexpected successful {action} reservation state")
        }
        Reservation::Reserved => format!("Unexpected reservation state for {action}"),
    })
}

/// `email_draft_create` output.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftOutput {
    pub draft_id: String,
    pub already_existed: bool,
    pub attachments: Vec<OutgoingAttachmentMetadata>,
}

/// `email_send` output.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendOutput {
    pub sent: bool,
    pub message_id: String,
    pub already_sent: bool,
    pub sent_copy: SentCopyReport,
    pub attachments: Vec<OutgoingAttachmentMetadata>,
}

/// `email_send_status` output.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendStatusOutput {
    pub found: bool,
    pub status: Option<AttemptStatus>,
    pub smtp_accepted: bool,
    pub message_id: Option<String>,
    pub recorded_at: Option<String>,
    pub message_date: Option<String>,
    pub sent_copy: Option<SentCopyReport>,
    pub attachments: Vec<OutgoingAttachmentMetadata>,
}

/// The compose workflows; cheap to clone.
#[derive(Clone)]
pub struct ComposeService {
    store: Store,
    clock: SharedClock,
    sender: Option<Arc<dyn ComposeSender>>,
    mailbox: Option<Arc<dyn ComposeMailbox>>,
    /// Re-reads referenced PDFs (the transport in production).
    attachments: Option<Arc<dyn AttachmentReader>>,
    /// The configured sender identity (`michael@thiesen.dev`); `None` when
    /// no SMTP configuration resolves.
    from: Option<String>,
}

impl ComposeService {
    pub fn new(
        store: Store,
        clock: SharedClock,
        sender: Option<Arc<dyn ComposeSender>>,
        mailbox: Option<Arc<dyn ComposeMailbox>>,
    ) -> Self {
        let from = sender
            .as_ref()
            .map(|_| omni_mailer::OUTGOING_EMAIL_FROM.to_owned());
        Self {
            store,
            clock,
            sender,
            mailbox,
            attachments: None,
            from,
        }
    }

    /// Enables attachments by pinned reference.
    #[must_use]
    pub fn with_attachment_reader(mut self, reader: Option<Arc<dyn AttachmentReader>>) -> Self {
        self.attachments = reader;
        self
    }

    fn mailbox(&self) -> Option<&Arc<dyn ComposeMailbox>> {
        self.mailbox.as_ref().filter(|m| m.available())
    }

    async fn reserve(
        &self,
        kind: ComposeKind,
        idempotency_key: &str,
        fingerprint: &str,
        prepared: Option<PreparedMessage>,
        attachments: Vec<OutgoingAttachmentMetadata>,
    ) -> Result<Reservation, EmailComposeError> {
        let pk = key_for(kind, idempotency_key);
        let fingerprint = fingerprint.to_owned();
        self.store
            .write(move |tx| -> Result<Reservation, StoreError> {
                let now = tx.now_ms();
                if let Some(prior) = read_attempt_tx(tx, &pk)? {
                    return Ok(existing_reservation(prior, &fingerprint));
                }
                let has_prepared = prepared.is_some();
                let attempt = StoredAttempt {
                    fingerprint: fingerprint.clone(),
                    status: AttemptStatus::Pending,
                    updated_at: now,
                    prepared,
                    sent_copy: has_prepared.then_some(SentCopyState::Pending),
                    attachments: (!attachments.is_empty()).then_some(attachments),
                    result: None,
                    extra: Extra::new(),
                };
                write_attempt(tx, &pk, kind, &attempt)?;
                Ok(Reservation::Reserved)
            })
            .await
            .map_err(fail_with(format!(
                "Could not reserve email {}",
                kind.as_str()
            )))
    }

    async fn complete(
        &self,
        kind: ComposeKind,
        idempotency_key: &str,
        fingerprint: &str,
        sent: bool,
        message_id: &str,
    ) -> Result<(), EmailComposeError> {
        let pk = key_for(kind, idempotency_key);
        let fingerprint = fingerprint.to_owned();
        let message_id = message_id.to_owned();
        self.store
            .write(move |tx| -> Result<(), CompleteError> {
                let now = tx.now_ms();
                let Some(mut prior) = read_attempt_tx(tx, &pk)? else {
                    return Err(CompleteError::Message(
                        "Email compose reservation disappeared",
                    ));
                };
                if prior.fingerprint == fingerprint
                    && prior.status == AttemptStatus::Succeeded
                    && prior
                        .result
                        .as_ref()
                        .is_some_and(|r| r.sent == sent && r.message_id == message_id)
                {
                    // A concurrent reconciler recorded the same outcome first.
                    return Ok(());
                }
                if prior.fingerprint != fingerprint || prior.status != AttemptStatus::Pending {
                    return Err(CompleteError::Message(
                        "Email compose reservation changed unexpectedly",
                    ));
                }
                prior.prepared = prior.prepared.map(|p| retained_mime(p, true));
                prior.status = if sent {
                    AttemptStatus::Succeeded
                } else {
                    AttemptStatus::Failed
                };
                prior.result = Some(AttemptResult {
                    sent,
                    message_id: message_id.clone(),
                    extra: Extra::new(),
                });
                prior.updated_at = now;
                write_attempt(tx, &pk, kind, &prior)?;
                Ok(())
            })
            .await
            .map_err(fail_with(format!(
                "Could not persist email {} outcome; do not repeat with a new key",
                kind.as_str()
            )))
    }

    async fn read_attempt(
        &self,
        kind: ComposeKind,
        idempotency_key: &str,
    ) -> Result<Option<StoredAttempt>, EmailComposeError> {
        let pk = key_for(kind, idempotency_key);
        self.store
            .read(move |docs| read_attempt_tx(docs, &pk))
            .await
            .map_err(fail_with(format!(
                "Could not read {} receipt",
                kind.as_str()
            )))
    }

    /// Settles known keys before re-reading attachment sources or composing MIME.
    async fn find_reservation(
        &self,
        kind: ComposeKind,
        idempotency_key: &str,
        fingerprint: &str,
    ) -> Result<Option<Reservation>, EmailComposeError> {
        Ok(self
            .read_attempt(kind, idempotency_key)
            .await?
            .map(|prior| existing_reservation(prior, fingerprint)))
    }

    async fn resolve_attachments(
        &self,
        request: &ComposeRequest,
    ) -> Result<Vec<crate::compose_attachments::ResolvedOutgoingAttachment>, EmailComposeError>
    {
        resolve_outgoing_attachments(
            self.attachments.as_ref(),
            request.attachments.as_deref().unwrap_or_default(),
        )
        .await
        .map_err(|e| EmailComposeError::new(e.0))
    }

    /// `email_draft_create`.
    pub async fn create_draft(
        &self,
        request: &ComposeRequest,
    ) -> Result<DraftOutput, EmailComposeError> {
        let Some(mailbox) = self.mailbox().cloned() else {
            return Err(EmailComposeError::new(
                "Email draft transport is not available",
            ));
        };
        let Some(from) = self.from.clone() else {
            return Err(EmailComposeError::new(
                "SMTP credentials for michael@thiesen.dev are not available",
            ));
        };
        let fingerprint = request.fingerprint(&from);
        let existing = self
            .find_reservation(ComposeKind::Draft, &request.idempotency_key, &fingerprint)
            .await?;
        let resolved = match existing {
            Some(_) => Vec::new(),
            None => self.resolve_attachments(request).await?,
        };
        let (files, metadata): (Vec<_>, Vec<_>) =
            resolved.into_iter().map(|r| (r.file, r.metadata)).unzip();
        let reservation = match existing {
            Some(existing) => existing,
            None => {
                self.reserve(
                    ComposeKind::Draft,
                    &request.idempotency_key,
                    &fingerprint,
                    None,
                    metadata.clone(),
                )
                .await?
            }
        };
        let (pending, attachments) = match reservation {
            Reservation::Succeeded {
                message_id,
                attachments,
            } => {
                return Ok(DraftOutput {
                    draft_id: message_id,
                    already_existed: true,
                    attachments,
                });
            }
            Reservation::Reserved => (false, metadata),
            Reservation::Pending { attachments } => (true, attachments),
            other => return Err(reservation_failure("draft", &other)),
        };
        let result = mailbox
            .create_draft(&request.draft_input(files), !pending)
            .await
            .map_err(|e| EmailComposeError::caused(e.to_string(), e))?;
        self.complete(
            ComposeKind::Draft,
            &request.idempotency_key,
            &fingerprint,
            true,
            &result.draft_id,
        )
        .await?;
        Ok(DraftOutput {
            draft_id: result.draft_id,
            already_existed: result.already_existed || pending,
            attachments,
        })
    }

    async fn already_sent(
        &self,
        idempotency_key: &str,
        message_id: String,
        attachments: Vec<OutgoingAttachmentMetadata>,
    ) -> Result<SendOutput, EmailComposeError> {
        Ok(SendOutput {
            sent: true,
            message_id,
            already_sent: true,
            sent_copy: self.save_sent(idempotency_key).await?,
            attachments,
        })
    }

    /// `email_send`.
    pub async fn send(&self, request: &ComposeRequest) -> Result<SendOutput, EmailComposeError> {
        let (Some(sender), Some(from)) = (self.sender.clone(), self.from.clone()) else {
            return Err(EmailComposeError::new(
                "SMTP credentials for michael@thiesen.dev are not configured",
            ));
        };
        let fingerprint = request.fingerprint(&from);
        match self
            .find_reservation(ComposeKind::Send, &request.idempotency_key, &fingerprint)
            .await?
        {
            Some(Reservation::Succeeded {
                message_id,
                attachments,
            }) => {
                return self
                    .already_sent(&request.idempotency_key, message_id, attachments)
                    .await;
            }
            Some(other) => return Err(reservation_failure("send", &other)),
            None => {}
        }
        let resolved = self.resolve_attachments(request).await?;
        let (files, metadata): (Vec<_>, Vec<_>) =
            resolved.into_iter().map(|r| (r.file, r.metadata)).unzip();
        let message_id = format!("<{fingerprint}@omni-notify>");
        let date_ms = self.clock.now_ms();
        let prepared = omni_mailer::prepare_composed_email(
            &request.compose_input(files),
            &message_id,
            date_ms,
        )
        .map_err(|e| EmailComposeError::caused(e.to_string(), e))?;
        let wire = STANDARD
            .decode(&prepared.wire_b64)
            .map_err(|e| EmailComposeError::caused(e.to_string(), e))?;
        let stored = PreparedMessage {
            from: prepared.from,
            date: prepared.date_iso,
            wire: Some(prepared.wire_b64),
            content: Some(prepared.content_b64),
            extra: Extra::new(),
        };
        let reservation = self
            .reserve(
                ComposeKind::Send,
                &request.idempotency_key,
                &fingerprint,
                Some(stored),
                metadata.clone(),
            )
            .await?;
        match reservation {
            Reservation::Succeeded {
                message_id,
                attachments,
            } => {
                return self
                    .already_sent(&request.idempotency_key, message_id, attachments)
                    .await;
            }
            Reservation::Reserved => {}
            other => return Err(reservation_failure("send", &other)),
        }
        let sent = sender.send(&request.recipients(), &wire).await;
        self.complete(
            ComposeKind::Send,
            &request.idempotency_key,
            &fingerprint,
            sent,
            &message_id,
        )
        .await?;
        if !sent {
            return Err(EmailComposeError::new(
                "Delivery was not confirmed for every recipient. Some recipients may have received the email; this operation will not be sent again automatically. Do not retry with a new key without checking delivery.",
            ));
        }
        Ok(SendOutput {
            sent: true,
            message_id,
            already_sent: false,
            sent_copy: self.save_sent(&request.idempotency_key).await?,
            attachments: metadata,
        })
    }

    /// `email_send_status`: never submits SMTP or appends mail.
    pub async fn send_status(
        &self,
        idempotency_key: &str,
    ) -> Result<SendStatusOutput, EmailComposeError> {
        let attempt = self
            .read_attempt(ComposeKind::Send, idempotency_key)
            .await?;
        Ok(match attempt {
            None => SendStatusOutput {
                found: false,
                status: None,
                smtp_accepted: false,
                message_id: None,
                recorded_at: None,
                message_date: None,
                sent_copy: None,
                attachments: Vec::new(),
            },
            Some(attempt) => SendStatusOutput {
                found: true,
                status: Some(attempt.status),
                smtp_accepted: attempt.status == AttemptStatus::Succeeded
                    && attempt.result.as_ref().is_some_and(|r| r.sent),
                message_id: attempt.result.as_ref().map(|r| r.message_id.clone()),
                recorded_at: Some(omni_core::js::to_iso_string(attempt.updated_at)),
                message_date: attempt.prepared.as_ref().map(|p| p.date.clone()),
                sent_copy: Some(
                    attempt
                        .sent_copy
                        .map_or(SentCopyReport::LegacyUnavailable, Into::into),
                ),
                attachments: attempt.attachments.unwrap_or_default(),
            },
        })
    }

    /// `email_sent_copy_repair`: never retransmits SMTP.
    pub async fn sent_copy_repair(
        &self,
        idempotency_key: &str,
    ) -> Result<SentCopyReport, EmailComposeError> {
        self.save_sent(idempotency_key).await
    }

    /// Claims APPEND durably; a crash or lost response permits read-only reconciliation only.
    async fn save_sent(&self, idempotency_key: &str) -> Result<SentCopyReport, EmailComposeError> {
        let attempt = self
            .read_attempt(ComposeKind::Send, idempotency_key)
            .await?;
        let confirmed = attempt.as_ref().filter(|a| {
            a.status == AttemptStatus::Succeeded && a.result.as_ref().is_some_and(|r| r.sent)
        });
        let Some(attempt) = confirmed else {
            return Err(EmailComposeError::new(
                "SMTP acceptance is not confirmed; Sent copy repair cannot send or assume delivery",
            ));
        };
        if attempt.sent_copy == Some(SentCopyState::Verified) {
            return Ok(SentCopyReport::Verified);
        }
        let Some((prepared, content)) = attempt
            .prepared
            .as_ref()
            .and_then(|p| p.content.as_ref().filter(|c| !c.is_empty()).map(|c| (p, c)))
        else {
            return Ok(SentCopyReport::LegacyUnavailable);
        };
        let Some(mailbox) = self.mailbox().cloned() else {
            return Ok(attempt
                .sent_copy
                .map_or(SentCopyReport::Pending, Into::into));
        };
        let message_id = attempt
            .result
            .as_ref()
            .map(|r| r.message_id.clone())
            .unwrap_or_default();
        self.copy_to_sent(
            idempotency_key,
            mailbox.as_ref(),
            message_id,
            content,
            &prepared.date,
            attempt.sent_copy == Some(SentCopyState::Pending),
        )
        .await
        .map_err(fail_with(
            "SMTP was accepted but Sent copy persistence failed; do not resend".to_owned(),
        ))
    }

    async fn copy_to_sent(
        &self,
        idempotency_key: &str,
        mailbox: &dyn ComposeMailbox,
        message_id: String,
        content: &str,
        date: &str,
        allow_append: bool,
    ) -> Result<SentCopyReport, EmailComposeError> {
        let content = STANDARD
            .decode(content)
            .map_err(|e| EmailComposeError::new(e.to_string()))?;
        let input = SentCopyInput {
            message_id,
            content,
            internal_date_ms: date
                .parse::<jiff::Timestamp>()
                .ok()
                .map(|t| t.as_millisecond()),
        };
        let pk = key_for(ComposeKind::Send, idempotency_key);
        let store = self.store.clone();
        let claim_pk = pk.clone();
        let before_append: BeforeAppend = Box::new(move || {
            Box::pin(async move {
                store
                    .write(move |tx| -> Result<bool, CompleteError> {
                        let Some(mut prior) = read_attempt_tx(tx, &claim_pk)? else {
                            return Err(CompleteError::Message("Send receipt disappeared"));
                        };
                        if prior.sent_copy != Some(SentCopyState::Pending) {
                            return Ok(false);
                        }
                        prior.sent_copy = Some(SentCopyState::Uncertain);
                        write_attempt(tx, &claim_pk, ComposeKind::Send, &prior)?;
                        Ok(true)
                    })
                    .await
                    .map_err(|e| e.to_string())
            })
        });
        let copied = mailbox
            .save_sent_copy(&input, allow_append, Some(before_append))
            .await;
        if let Err(error) = copied {
            tracing::warn!(target: LOG, "Sent copy not confirmed: {error}");
            let latest = self
                .read_attempt(ComposeKind::Send, idempotency_key)
                .await?;
            return Ok(latest
                .and_then(|a| a.sent_copy)
                .map_or(SentCopyReport::Pending, Into::into));
        }
        self.store
            .write(move |tx| -> Result<(), CompleteError> {
                let Some(mut prior) = read_attempt_tx(tx, &pk)? else {
                    return Err(CompleteError::Message("Send receipt disappeared"));
                };
                prior.prepared = prior.prepared.map(|p| retained_mime(p, false));
                prior.sent_copy = Some(SentCopyState::Verified);
                write_attempt(tx, &pk, ComposeKind::Send, &prior)?;
                Ok(())
            })
            .await
            .map_err(|e| EmailComposeError::new(e.to_string()))?;
        Ok(SentCopyReport::Verified)
    }
}

/// A failure inside a compose transaction.
#[derive(Debug, thiserror::Error)]
enum CompleteError {
    #[error("{0}")]
    Message(&'static str),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl ComposeMailbox for crate::transport::ImapTransport {
    fn available(&self) -> bool {
        self.is_active()
    }

    fn create_draft<'a>(
        &'a self,
        input: &'a EmailDraftInput,
        allow_append: bool,
    ) -> BoxFuture<'a, Result<EmailDraftResult, ImapError>> {
        Box::pin(crate::transport::ImapTransport::create_draft(
            self,
            input,
            allow_append,
        ))
    }

    fn save_sent_copy<'a>(
        &'a self,
        input: &'a SentCopyInput,
        allow_append: bool,
        before_append: Option<BeforeAppend>,
    ) -> BoxFuture<'a, Result<SentCopyResult, ImapError>> {
        Box::pin(crate::transport::ImapTransport::save_sent_copy(
            self,
            input,
            allow_append,
            before_append,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reference value from zod + `JSON.stringify` + sha256 in node.
    #[test]
    fn fingerprint_matches_the_ts_zod_output() {
        let request = ComposeRequest {
            idempotency_key: "k1".to_owned(),
            to: vec!["a@example.test".to_owned()],
            cc: None,
            bcc: Some(vec!["b@example.test".to_owned()]),
            subject: "Hi".to_owned(),
            text: "Body ".to_owned(),
            in_reply_to: Some("<p@x.y>".to_owned()),
            references: Some(vec!["<r@x.y>".to_owned()]),
            attachments: None,
        };
        assert_eq!(
            request.fingerprint_json("michael@thiesen.dev"),
            r#"{"idempotencyKey":"k1","to":["a@example.test"],"bcc":["b@example.test"],"subject":"Hi","text":"Body ","inReplyTo":"<p@x.y>","references":["<r@x.y>"],"from":"michael@thiesen.dev"}"#
        );
        assert_eq!(
            request.fingerprint("michael@thiesen.dev"),
            "50b5ca2eab0a7bb427ec71542cf53acde9089b05f858a437b5a9381de29191ba"
        );
    }
}
