//! Transport-agnostic incoming email model, shared by
//! the mail transport, the email pipelines, workspaces and MCP, and the handler
//! trait the dispatcher fans out to.

use serde::{Deserialize, Serialize};

use crate::{BoxError, BoxFuture};

/// One fetched message. Field names and optionality are its persisted shape.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchedEmail {
    /// Stable identity: Message-ID with angle brackets, or `imap|<folder>|<uv>|<uid>`.
    pub id: String,
    /// Exact mailbox coordinates at the time of this read; a later move changes them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<EmailOrigin>,
    pub subject: String,
    pub from: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cc: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub references: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_reply_to: Option<String>,
    pub text_body: String,
    /// Shipment/booking-shaped URLs pulled from the HTML body.
    pub links: Vec<String>,
    /// Untrusted private link targets. Never log or include in routine reports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_metadata: Option<EmailLinkMetadata>,
    /// ISO-8601 with milliseconds (`Date#toISOString`).
    pub received_at: String,
    pub attachments: Vec<EmailAttachment>,
}

/// IMAP coordinates of a message at read time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailOrigin {
    pub folder: String,
    pub uid_validity: String,
    pub uid: u32,
}

/// Attachment metadata (no bytes).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailAttachment {
    /// Opaque IMAP folder and UID coordinates.
    pub blob_id: String,
    /// Stable exact Message-ID plus MIME part handle, independent of folder/UID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disposition: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_id: Option<String>,
    pub name: String,
    /// MIME type (wire field `type`).
    #[serde(rename = "type")]
    pub mime_type: String,
    pub size: u64,
}

/// A downloaded attachment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DownloadedAttachment {
    pub name: String,
    pub mime_type: String,
    pub data: Vec<u8>,
}

/// Verified bytes re-read from the mailbox by stable identity, never
/// caller-supplied (`OutgoingEmailAttachment`). `content_type` is always
/// `application/pdf`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutgoingEmailAttachment {
    pub filename: String,
    pub content_type: String,
    pub content: Vec<u8>,
}

/// Links extracted from a message body.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailLinkMetadata {
    pub links: Vec<EmailLink>,
    pub links_truncated: bool,
    pub list_unsubscribe: ListUnsubscribe,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailLink {
    pub url: String,
    pub label: String,
    pub source: EmailLinkSource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EmailLinkSource {
    #[serde(rename = "html")]
    Html,
    #[serde(rename = "text")]
    Text,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListUnsubscribe {
    pub urls: Vec<String>,
    /// Only `"List-Unsubscribe=One-Click"` or `null`.
    pub post: Option<String>,
    pub present: bool,
    pub truncated: bool,
}

/// A pipeline that consumes dispatched emails.
pub trait EmailHandler: Send + Sync {
    /// `"McpEvents" | "ParcelTracker" | "CalendarEvents" | "Workspaces"`.
    fn name(&self) -> &'static str;
    fn handle<'a>(&'a self, emails: &'a [FetchedEmail]) -> BoxFuture<'a, Result<(), HandlerError>>;
}

/// A handler failure; transient failures are retried by the email retry queue.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct HandlerError {
    pub message: String,
    pub transient: bool,
    #[source]
    pub source: Option<BoxError>,
}

impl HandlerError {
    /// A failure worth retrying (network, 5xx).
    pub fn transient(message: impl Into<String>, source: Option<BoxError>) -> Self {
        Self {
            message: message.into(),
            transient: true,
            source,
        }
    }

    /// A failure that replaying will not fix.
    pub fn permanent(message: impl Into<String>, source: Option<BoxError>) -> Self {
        Self {
            message: message.into(),
            transient: false,
            source,
        }
    }
}
