//! Drafts in the server-designated `\Drafts` mailbox.
//! The deterministic Message-ID lets an uncertain APPEND be reconciled by
//! search before (or instead of) another APPEND.

use mail_builder::MessageBuilder;
use mail_builder::headers::address::Address as MimeAddress;
use mail_builder::headers::message_id::MessageId;
use sha2::{Digest as _, Sha256};

use crate::protocol::{FetchQuery, ImapClient, ImapError, SearchCriteria, fetch_one};

/// `EmailDraftInput`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EmailDraftInput {
    pub idempotency_key: String,
    pub to: Vec<String>,
    pub cc: Option<Vec<String>>,
    pub bcc: Option<Vec<String>>,
    pub subject: String,
    pub text: String,
    pub in_reply_to: Option<String>,
    pub references: Option<Vec<String>>,
    /// Every entry must be appended; an implementation that cannot must fail.
    pub attachments: Vec<omni_mailer::OutgoingEmailAttachment>,
}

fn bare_id(id: &str) -> String {
    id.trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .to_owned()
}

fn address_list(addresses: &[String]) -> MimeAddress<'static> {
    MimeAddress::new_list(
        addresses
            .iter()
            .map(|a| MimeAddress::new_address(None::<String>, a.clone()))
            .collect(),
    )
}

/// The draft MIME (MailComposer with `keepBcc`): the fixed sender, Bcc kept,
/// references exactly as given, Date = now.
pub fn compose_draft_mime(
    input: &EmailDraftInput,
    message_id: &str,
    now_ms: i64,
) -> Result<Vec<u8>, String> {
    let mut builder = MessageBuilder::new()
        .from(MimeAddress::new_address(
            None::<String>,
            omni_mailer::OUTGOING_EMAIL_FROM,
        ))
        .message_id(bare_id(message_id))
        .date(now_ms.div_euclid(1000))
        .subject(input.subject.clone());
    if !input.to.is_empty() {
        builder = builder.to(address_list(&input.to));
    }
    if let Some(cc) = input.cc.as_ref().filter(|cc| !cc.is_empty()) {
        builder = builder.cc(address_list(cc));
    }
    if let Some(bcc) = input.bcc.as_ref().filter(|bcc| !bcc.is_empty()) {
        builder = builder.bcc(address_list(bcc));
    }
    if let Some(parent) = &input.in_reply_to {
        builder = builder.in_reply_to(bare_id(parent));
    }
    if let Some(references) = input.references.as_ref().filter(|r| !r.is_empty()) {
        builder = builder.references(MessageId::new_list(references.iter().map(|r| bare_id(r))));
    }
    omni_mailer::with_attachments(builder.text_body(input.text.clone()), &input.attachments)
        .write_to_vec()
        .map_err(|e| e.to_string())
}

/// `EmailDraftResult`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmailDraftResult {
    /// Stable Message-ID used to reconcile an uncertain APPEND.
    pub draft_id: String,
    pub already_existed: bool,
}

/// `<sha256hex(idempotencyKey)@omni-notify>`.
pub fn deterministic_draft_message_id(idempotency_key: &str) -> String {
    format!(
        "<{}@omni-notify>",
        hex::encode(Sha256::digest(idempotency_key.as_bytes()))
    )
}

fn err(operation: &str, detail: impl Into<String>) -> ImapError {
    ImapError::new(operation, detail)
}

async fn has_exact_draft(
    client: &mut dyn ImapClient,
    message_id: &str,
    operation: &str,
    fetch_operation: &str,
) -> Result<bool, ImapError> {
    let matches = client
        .uid_search(&SearchCriteria::message_id(message_id))
        .await
        .map_err(|e| ImapError::wrap(operation, e))?;
    for uid in matches.unwrap_or_default() {
        let found = fetch_one(client, uid, FetchQuery::envelope())
            .await
            .map_err(|e| ImapError::wrap(fetch_operation, e))?;
        if found
            .and_then(|f| f.envelope_message_id)
            .is_some_and(|id| id.to_lowercase() == message_id.to_lowercase())
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Resolves an uncertain prior APPEND before creating a draft with the same key.
pub async fn create_draft(
    client: &mut dyn ImapClient,
    input: &EmailDraftInput,
    allow_append: bool,
    now_ms: i64,
) -> Result<EmailDraftResult, ImapError> {
    let message_id = deterministic_draft_message_id(&input.idempotency_key);
    let folders = client
        .list()
        .await
        .map_err(|e| ImapError::wrap("LIST mailboxes", e))?;
    let Some(mailbox) = folders.iter().find(|f| f.special_use_is("\\drafts")) else {
        return Err(err(
            "discover Drafts mailbox",
            "IMAP server did not designate a Drafts mailbox",
        ));
    };
    let path = mailbox.path.clone();
    client
        .select(&path, false)
        .await
        .map_err(|e| ImapError::wrap("open Drafts mailbox", e))?;

    if has_exact_draft(client, &message_id, "search draft", "fetch matching draft").await? {
        return Ok(EmailDraftResult {
            draft_id: message_id,
            already_existed: true,
        });
    }
    if !allow_append {
        return Err(err(
            "reconcile prior draft APPEND",
            "No matching draft is visible; the prior APPEND will not be repeated",
        ));
    }

    let content = compose_draft_mime(input, &message_id, now_ms)
        .map_err(|e| ImapError::new("compose draft", e))?;
    let appended = client
        .append(&path, &content, &["\\Draft"], None)
        .await
        .map_err(|e| ImapError::wrap("APPEND draft", e))?;
    if !appended {
        return Err(err(
            "APPEND draft",
            "IMAP server did not confirm the APPEND",
        ));
    }
    if !has_exact_draft(
        client,
        &message_id,
        "verify appended draft",
        "verify appended draft",
    )
    .await?
    {
        return Err(err(
            "verify appended draft",
            "APPEND completed but the deterministic Message-ID is not yet visible",
        ));
    }
    Ok(EmailDraftResult {
        draft_id: message_id,
        already_existed: false,
    })
}
