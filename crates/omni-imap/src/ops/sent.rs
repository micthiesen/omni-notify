//! Private Sent copies (`src/email/imap/sent.ts`): discover the uniquely
//! designated `\Sent` mailbox, reconcile by exact Message-ID and semantic MIME,
//! APPEND the unchanged original MIME with its original INTERNALDATE only after
//! a durable caller claim, and verify the copy. An uncertain APPEND is only
//! ever searched, never repeated.

use futures::future::BoxFuture;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use crate::mime::{Address, ParsedMail, parse_message};
use crate::protocol::{FetchQuery, ImapClient, ImapError, SearchCriteria, SourceRange, fetch_one};

const MAX_CANDIDATES: usize = 50;

/// `SentCopyInput`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SentCopyInput {
    pub message_id: String,
    pub content: Vec<u8>,
    /// The original send time (epoch ms); `None` is an invalid date.
    pub internal_date_ms: Option<i64>,
}

/// `SentCopyResult`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SentCopyResult {
    pub message_id: String,
    pub mailbox: String,
    pub already_existed: bool,
}

/// The durable APPEND claim, run after the duplicate lookup. `Ok(false)` means
/// another attempt owns APPEND.
pub type BeforeAppend = Box<dyn FnOnce() -> BoxFuture<'static, Result<bool, String>> + Send>;

fn failure(operation: &str, detail: &str) -> ImapError {
    ImapError::new(operation, detail)
}

fn address_json(groups: &[&Address]) -> Vec<String> {
    groups
        .iter()
        .map(|a| {
            omni_core::js::json_stringify(&json!([a.address.clone().unwrap_or_default(), a.name]))
        })
        .collect()
}

fn single(list: Option<&Vec<Address>>) -> Vec<&Address> {
    list.map(|l| l.iter().collect()).unwrap_or_default()
}

fn multi(lists: Option<&Vec<Vec<Address>>>) -> Vec<&Address> {
    lists
        .map(|l| l.iter().flatten().collect())
        .unwrap_or_default()
}

/// Decoded MIME content for comparison, so harmless wire line-ending changes
/// are allowed (`semanticMessage`).
pub(crate) fn semantic_message(mail: &ParsedMail) -> String {
    let attachments: Vec<Value> = mail
        .attachments
        .iter()
        .map(|a| {
            json!({
                "name": a.filename,
                "type": a.content_type,
                "disposition": a.content_disposition,
                "cid": a.cid,
                "digest": hex::encode(Sha256::digest(&a.content)),
            })
        })
        .collect();
    let value = json!({
        "messageId": mail.message_id,
        "date": mail.date.map(omni_core::js::to_iso_string),
        "from": address_json(&single(mail.from.as_ref())),
        "to": address_json(&multi(mail.to.as_ref())),
        "cc": address_json(&multi(mail.cc.as_ref())),
        "bcc": address_json(&multi(mail.bcc.as_ref())),
        "replyTo": address_json(&single(mail.reply_to.as_ref())),
        "subject": mail.subject,
        "text": mail.text.as_ref().map(|t| t.replace("\r\n", "\n")),
        "html": mail.html,
        "inReplyTo": mail.in_reply_to,
        "references": mail.references.clone().unwrap_or_default(),
        "attachments": attachments,
    });
    omni_core::js::json_stringify(&value)
}

async fn discover_sent(client: &mut dyn ImapClient) -> Result<String, ImapError> {
    let folders = client
        .list()
        .await
        .map_err(|e| ImapError::wrap("LIST mailboxes", e))?;
    let sent: Vec<_> = folders
        .iter()
        .filter(|f| f.special_use_is("\\sent"))
        .collect();
    match sent.len() {
        1 => Ok(sent[0].path.clone()),
        0 => Err(failure(
            "discover Sent mailbox",
            "IMAP server did not designate a Sent mailbox",
        )),
        _ => Err(failure(
            "discover Sent mailbox",
            "IMAP server designated multiple Sent mailboxes",
        )),
    }
}

async fn find_in_selected_sent(
    client: &mut dyn ImapClient,
    mailbox: &str,
    message_id: &str,
    expected: Option<&str>,
    now_ms: i64,
) -> Result<Option<SentCopyResult>, ImapError> {
    let matches = client
        .uid_search(&SearchCriteria::message_id(message_id))
        .await
        .map_err(|e| ImapError::wrap("search Sent copy", e))?;
    let matches = matches.unwrap_or_default();
    if matches.len() > MAX_CANDIDATES {
        return Err(failure(
            "search Sent copy",
            &format!(
                "More than {MAX_CANDIDATES} candidates; refusing an incomplete duplicate check"
            ),
        ));
    }
    let mut found = None;
    for uid in matches {
        let query = FetchQuery {
            envelope: true,
            source: Some(SourceRange::Full),
            ..FetchQuery::default()
        };
        let message = fetch_one(client, uid, query)
            .await
            .map_err(|e| ImapError::wrap("fetch matching Sent copy", e))?;
        // IMAP HEADER searches are substring matches. Compare the full identifier.
        let Some(message) =
            message.filter(|m| m.envelope_message_id.as_deref() == Some(message_id))
        else {
            continue;
        };
        if let Some(expected) = expected {
            let Some(source) = message.source else {
                return Err(failure(
                    "verify Sent copy",
                    "Matching Message-ID has no readable MIME source",
                ));
            };
            let parsed = parse_message(&source, now_ms)
                .map_err(|e| ImapError::new("parse Sent copy", e.to_string()))?;
            if semantic_message(&parsed) != expected {
                return Err(failure(
                    "verify Sent copy",
                    "Matching Message-ID belongs to different MIME content",
                ));
            }
        }
        found = Some(SentCopyResult {
            message_id: message_id.to_owned(),
            mailbox: mailbox.to_owned(),
            already_existed: true,
        });
    }
    Ok(found)
}

/// Read-only duplicate lookup in the uniquely designated Sent mailbox.
pub async fn find_sent_copy(
    client: &mut dyn ImapClient,
    message_id: &str,
    now_ms: i64,
) -> Result<Option<SentCopyResult>, ImapError> {
    let mailbox = discover_sent(client).await?;
    client
        .select(&mailbox, true)
        .await
        .map_err(|e| ImapError::wrap("open Sent mailbox", e))?;
    find_in_selected_sent(client, &mailbox, message_id, None, now_ms).await
}

/// Appends only after a durable caller reservation; uncertain APPENDs are read-only.
pub async fn append_sent_copy(
    client: &mut dyn ImapClient,
    input: &SentCopyInput,
    allow_append: bool,
    before_append: Option<BeforeAppend>,
    now_ms: i64,
) -> Result<SentCopyResult, ImapError> {
    let Some(internal_date) = input.internal_date_ms else {
        return Err(failure(
            "prepare Sent copy",
            "Invalid original internal date",
        ));
    };
    let parsed = parse_message(&input.content, now_ms)
        .map_err(|e| ImapError::new("parse original Sent MIME", e.to_string()))?;
    if parsed.message_id.as_deref() != Some(input.message_id.as_str())
        || parsed.date.is_none()
        || parsed.from.is_none()
    {
        return Err(failure(
            "prepare Sent copy",
            "Original MIME must preserve the exact Message-ID, Date, and From headers",
        ));
    }
    let expected = semantic_message(&parsed);
    let mailbox = discover_sent(client).await?;
    client
        .select(&mailbox, !allow_append)
        .await
        .map_err(|e| ImapError::wrap("open Sent mailbox", e))?;
    if let Some(prior) =
        find_in_selected_sent(client, &mailbox, &input.message_id, Some(&expected), now_ms).await?
    {
        return Ok(prior);
    }
    if !allow_append {
        return Err(failure(
            "reconcile prior Sent APPEND",
            "No matching Sent copy is visible; the prior APPEND will not be repeated",
        ));
    }
    if let Some(claim) = before_append {
        let allowed = claim()
            .await
            .map_err(|cause| ImapError::new("reserve Sent APPEND", cause))?;
        if !allowed {
            return Err(failure(
                "reserve Sent APPEND",
                "Another attempt owns APPEND; only reconcile the existing copy",
            ));
        }
    }
    let appended = client
        .append(&mailbox, &input.content, &["\\Seen"], Some(internal_date))
        .await
        .map_err(|e| ImapError::wrap("APPEND Sent copy", e))?;
    if !appended {
        return Err(failure(
            "APPEND Sent copy",
            "IMAP server did not confirm APPEND; reconcile before another attempt",
        ));
    }
    let verified =
        find_in_selected_sent(client, &mailbox, &input.message_id, Some(&expected), now_ms).await?;
    let Some(verified) = verified else {
        return Err(failure(
            "verify Sent APPEND",
            "APPEND completed but matching MIME is not visible; reconcile before another attempt",
        ));
    };
    Ok(SentCopyResult {
        already_existed: false,
        ..verified
    })
}
