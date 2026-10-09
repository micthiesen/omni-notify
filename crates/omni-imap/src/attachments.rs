//! Stable, read-only attachment identity.

use std::sync::LazyLock;

use futures::future::BoxFuture;
use omni_core::email::DownloadedAttachment;
use regex::Regex;
use sha2::{Digest as _, Sha256};

use crate::mime::Attachment;
use crate::protocol::ImapError;

/// Decoded attachment bytes returned to callers.
pub const MAX_ATTACHMENT_BYTES: usize = 5 * 1024 * 1024;
/// Source message bytes read for an attachment lookup.
pub const MAX_ATTACHMENT_MESSAGE_BYTES: usize = 20 * 1024 * 1024;

fn regex(pattern: &str) -> Option<Regex> {
    Regex::new(pattern).ok()
}

static MESSAGE_ID: LazyLock<Option<Regex>> =
    LazyLock::new(|| regex(r"^<[^<>\s\x00-\x1f\x7f]+@[^<>\s\x00-\x1f\x7f]+>$"));
static PART_ID: LazyLock<Option<Regex>> = LazyLock::new(|| regex(r"^[1-9]\d*(?:\.[1-9]\d*)*$"));
static MIME_TYPE: LazyLock<Option<Regex>> =
    LazyLock::new(|| regex(r"(?i)^[a-z0-9!#$&^_.+-]+/[a-z0-9!#$&^_.+-]+$"));
static STABLE_ID: LazyLock<Option<Regex>> =
    LazyLock::new(|| regex(r"^imap-attachment:[a-f0-9]{64}$"));

fn is_match(re: &LazyLock<Option<Regex>>, s: &str) -> bool {
    re.as_ref().is_some_and(|re| re.is_match(s))
}

/// An exact bracketed RFC Message-ID of at most 998 UTF-16 units.
pub fn valid_attachment_message_id(message_id: &str) -> bool {
    omni_core::js::utf16_len(message_id) <= 998 && is_match(&MESSAGE_ID, message_id)
}

/// `imap-attachment:<64 hex>`.
pub fn valid_stable_attachment_id(attachment_id: &str) -> bool {
    is_match(&STABLE_ID, attachment_id)
}

/// mailparser's MIME tree coordinate: a root part without a parent boundary is `1`.
pub fn attachment_part_id(attachment: &Attachment) -> Option<String> {
    match &attachment.part_id {
        None => Some("1".to_owned()),
        Some(id) if is_match(&PART_ID, id) => Some(id.clone()),
        Some(_) => None,
    }
}

/// `imap-attachment:` + sha256(JSON.stringify([messageId, partId])).
pub fn encode_stable_attachment_id(message_id: &str, part_id: &str) -> String {
    let json = omni_core::js::json_stringify(&serde_json::json!([message_id, part_id]));
    format!(
        "imap-attachment:{}",
        hex::encode(Sha256::digest(json.as_bytes()))
    )
}

/// The declared `Content-Type` (never inferred from a filename), lowercased,
/// or `application/octet-stream`.
pub fn declared_attachment_mime_type(attachment: &Attachment) -> String {
    match &attachment.declared_content_type {
        Some(value) if is_match(&MIME_TYPE, value) => value.to_lowercase(),
        _ => "application/octet-stream".to_owned(),
    }
}

/// Format identity only: declared PDF MIME plus a PDF header, not document safety.
pub fn is_pdf_attachment(mime_type: &str, data: &[u8]) -> bool {
    mime_type.eq_ignore_ascii_case("application/pdf") && data.starts_with(b"%PDF-")
}

/// The stable attachment reader seam (the IMAP transport in production).
pub trait AttachmentReader: Send + Sync {
    /// False until the transport has started (TS `emailControls.transport` unset).
    fn available(&self) -> bool {
        true
    }
    fn fetch_attachment<'a>(
        &'a self,
        message_id: &'a str,
        attachment_id: &'a str,
        max_bytes: usize,
    ) -> BoxFuture<'a, Result<Option<DownloadedAttachment>, ImapError>>;
}

/// A display/download filename, never a sender-supplied filesystem path.
pub fn safe_attachment_filename(filename: Option<&str>) -> String {
    let raw = filename.unwrap_or("attachment").replace('\\', "/");
    let last = raw.rsplit('/').next().unwrap_or("");
    let cleaned: String = last
        .chars()
        .filter(|c| {
            let code = u32::from(*c);
            !(code <= 0x1f
                || (0x7f..=0x9f).contains(&code)
                || code == 0x061c
                || (0x200b..=0x200f).contains(&code)
                || (0x202a..=0x202e).contains(&code)
                || (0x2066..=0x2069).contains(&code)
                || code == 0xfeff)
        })
        .collect();
    let trimmed = omni_core::js::trim(&cleaned);
    let without_dots = trimmed.trim_start_matches('.');
    let limited = omni_core::js::utf16_slice(without_dots, 0, 180).into_owned();
    if limited.is_empty() {
        "attachment".to_owned()
    } else {
        limited
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_id_hashes_js_json() {
        let id = encode_stable_attachment_id("<a@b>", "2.1");
        assert!(valid_stable_attachment_id(&id));
        let expected = hex::encode(Sha256::digest(br#"["<a@b>","2.1"]"#));
        assert_eq!(id, format!("imap-attachment:{expected}"));
    }

    #[test]
    fn message_id_validation() {
        assert!(valid_attachment_message_id("<attachment@example.test>"));
        assert!(!valid_attachment_message_id("bad\r\nidentity"));
        assert!(!valid_attachment_message_id("<no-at>"));
    }
}
