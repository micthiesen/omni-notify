//! Durable compose receipts and workflows behind `email_draft_create`,
//! `email_send`, `email_send_status` and `email_sent_copy_repair`
//! (`src/mcp/tools/email-compose.ts`).
//!
//! Drafts and sends reserve their idempotency key durably before any side
//! effect. Uncertain sends are never retried; a partial recipient rejection is
//! never success. Composed sends persist the exact MIME before SMTP, record
//! SMTP acceptance before the Sent APPEND, and keep Sent verification separate
//! from delivery. Raw keys: `email-compose:{draft|send}:<sha256hex(key)>`.

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
/// Bcc) and `content` (private Sent copy).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparedMessage {
    pub from: String,
    /// ISO send date.
    pub date: String,
    pub wire: String,
    pub content: String,
    #[serde(flatten)]
    pub extra: Extra,
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
        map.insert("from".to_owned(), Value::from(from));
        omni_core::js::json_stringify(&Value::Object(map))
    }

    /// `sha256(JSON.stringify({...input, from}))`.
    pub fn fingerprint(&self, from: &str) -> String {
        hex::encode(Sha256::digest(self.fingerprint_json(from).as_bytes()))
    }

    fn draft_input(&self) -> EmailDraftInput {
        EmailDraftInput {
            idempotency_key: self.idempotency_key.clone(),
            to: self.to.clone(),
            cc: self.cc.clone(),
            bcc: self.bcc.clone(),
            subject: self.subject.clone(),
            text: self.text.clone(),
            in_reply_to: self.in_reply_to.clone(),
            references: self.references.clone(),
        }
    }

    fn compose_input(&self) -> omni_mailer::ComposeInput {
        omni_mailer::ComposeInput {
            to: self.to.clone(),
            cc: self.cc.clone().unwrap_or_default(),
            bcc: self.bcc.clone().unwrap_or_default(),
            subject: self.subject.clone(),
            text: self.text.clone(),
            in_reply_to: self.in_reply_to.clone(),
            references: self.references.clone().unwrap_or_default(),
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
    Succeeded(String),
    Pending,
    Failed,
    Mismatch,
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
        Reservation::Pending => format!(
            "A prior {action} attempt has an uncertain outcome; it will not be retried automatically. Do not retry with a new key without checking delivery."
        ),
        Reservation::Failed => format!(
            "A prior {action} attempt was not confirmed and will not be retried automatically. Do not retry with a new key without checking delivery."
        ),
        Reservation::Succeeded(_) => format!("Unexpected successful {action} reservation state"),
        Reservation::Reserved => format!("Unexpected reservation state for {action}"),
    })
}

/// `email_draft_create` output.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftOutput {
    pub draft_id: String,
    pub already_existed: bool,
}

/// `email_send` output.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendOutput {
    pub sent: bool,
    pub message_id: String,
    pub already_sent: bool,
    pub sent_copy: SentCopyReport,
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
}

/// The compose workflows; cheap to clone.
#[derive(Clone)]
pub struct ComposeService {
    store: Store,
    clock: SharedClock,
    sender: Option<Arc<dyn ComposeSender>>,
    mailbox: Option<Arc<dyn ComposeMailbox>>,
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
            from,
        }
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
    ) -> Result<Reservation, EmailComposeError> {
        let pk = key_for(kind, idempotency_key);
        let fingerprint = fingerprint.to_owned();
        self.store
            .write(move |tx| -> Result<Reservation, StoreError> {
                let now = tx.now_ms();
                if let Some(prior) = read_attempt_tx(tx, &pk)? {
                    if prior.fingerprint != fingerprint {
                        return Ok(Reservation::Mismatch);
                    }
                    return Ok(match (prior.status, prior.result) {
                        (AttemptStatus::Succeeded, Some(result)) if result.sent => {
                            Reservation::Succeeded(result.message_id)
                        }
                        (AttemptStatus::Succeeded, _) | (AttemptStatus::Failed, _) => {
                            Reservation::Failed
                        }
                        (AttemptStatus::Pending, _) => Reservation::Pending,
                    });
                }
                let has_prepared = prepared.is_some();
                let attempt = StoredAttempt {
                    fingerprint: fingerprint.clone(),
                    status: AttemptStatus::Pending,
                    updated_at: now,
                    prepared,
                    sent_copy: has_prepared.then_some(SentCopyState::Pending),
                    result: None,
                    extra: Extra::new(),
                };
                write_attempt(tx, &pk, kind, &attempt)?;
                Ok(Reservation::Reserved)
            })
            .await
            .map_err(|e| {
                EmailComposeError::caused(format!("Could not reserve email {}", kind.as_str()), e)
            })
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
                if prior.fingerprint != fingerprint || prior.status != AttemptStatus::Pending {
                    return Err(CompleteError::Message(
                        "Email compose reservation changed unexpectedly",
                    ));
                }
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
            .map_err(|e| {
                EmailComposeError::caused(
                    format!(
                        "Could not persist email {} outcome; do not repeat with a new key",
                        kind.as_str()
                    ),
                    e,
                )
            })
    }

    async fn read_send_attempt(
        &self,
        idempotency_key: &str,
    ) -> Result<Option<StoredAttempt>, EmailComposeError> {
        let pk = key_for(ComposeKind::Send, idempotency_key);
        self.store
            .read(move |docs| read_attempt_tx(docs, &pk))
            .await
            .map_err(|e| EmailComposeError::caused("Could not read send receipt", e))
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
        let reservation = self
            .reserve(
                ComposeKind::Draft,
                &request.idempotency_key,
                &fingerprint,
                None,
            )
            .await?;
        match &reservation {
            Reservation::Succeeded(message_id) => {
                return Ok(DraftOutput {
                    draft_id: message_id.clone(),
                    already_existed: true,
                });
            }
            Reservation::Reserved | Reservation::Pending => {}
            other => return Err(reservation_failure("draft", other)),
        }
        let pending = reservation == Reservation::Pending;
        let result = mailbox
            .create_draft(&request.draft_input(), !pending)
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
        let message_id = format!("<{fingerprint}@omni-notify>");
        let date_ms = self.clock.now_ms();
        let prepared =
            omni_mailer::prepare_composed_email(&request.compose_input(), &message_id, date_ms)
                .map_err(|e| EmailComposeError::caused(e.to_string(), e))?;
        let wire = STANDARD
            .decode(&prepared.wire_b64)
            .map_err(|e| EmailComposeError::caused(e.to_string(), e))?;
        let stored = PreparedMessage {
            from: prepared.from.clone(),
            date: prepared.date_iso.clone(),
            wire: prepared.wire_b64.clone(),
            content: prepared.content_b64.clone(),
            extra: Extra::new(),
        };
        let reservation = self
            .reserve(
                ComposeKind::Send,
                &request.idempotency_key,
                &fingerprint,
                Some(stored),
            )
            .await?;
        match reservation {
            Reservation::Succeeded(message_id) => {
                return Ok(SendOutput {
                    sent: true,
                    message_id,
                    already_sent: true,
                    sent_copy: self.save_sent(&request.idempotency_key).await?,
                });
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
        })
    }

    /// `email_send_status`: never submits SMTP or appends mail.
    pub async fn send_status(
        &self,
        idempotency_key: &str,
    ) -> Result<SendStatusOutput, EmailComposeError> {
        let attempt = self.read_send_attempt(idempotency_key).await?;
        Ok(match attempt {
            None => SendStatusOutput {
                found: false,
                status: None,
                smtp_accepted: false,
                message_id: None,
                recorded_at: None,
                message_date: None,
                sent_copy: None,
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
        self.save_sent_inner(idempotency_key)
            .await
            .map_err(|cause| {
                EmailComposeError::caused(
                    "SMTP was accepted but Sent copy persistence failed; do not resend",
                    cause,
                )
            })
    }

    async fn save_sent_inner(
        &self,
        idempotency_key: &str,
    ) -> Result<SentCopyReport, EmailComposeError> {
        let attempt = self.read_send_attempt(idempotency_key).await?;
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
        let Some(prepared) = &attempt.prepared else {
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
        let content = STANDARD
            .decode(&prepared.content)
            .map_err(|e| EmailComposeError::caused(e.to_string(), e))?;
        let input = SentCopyInput {
            message_id,
            content,
            internal_date_ms: prepared
                .date
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
        let allow_append = attempt.sent_copy == Some(SentCopyState::Pending);
        let copied = mailbox
            .save_sent_copy(&input, allow_append, Some(before_append))
            .await;
        if let Err(error) = copied {
            tracing::warn!(target: LOG, "Sent copy not confirmed: {error}");
            let latest = self.read_send_attempt(idempotency_key).await?;
            return Ok(latest
                .and_then(|a| a.sent_copy)
                .map_or(SentCopyReport::Pending, Into::into));
        }
        self.store
            .write(move |tx| -> Result<(), CompleteError> {
                let Some(mut prior) = read_attempt_tx(tx, &pk)? else {
                    return Err(CompleteError::Message("Send receipt disappeared"));
                };
                prior.sent_copy = Some(SentCopyState::Verified);
                write_attempt(tx, &pk, ComposeKind::Send, &prior)?;
                Ok(())
            })
            .await
            .map_err(|e| EmailComposeError::caused(e.to_string(), e))?;
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
