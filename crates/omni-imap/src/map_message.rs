//! Parsed message to the pipeline shape (`src/email/imap/mapMessage.ts`).
//!
//! The HTML-to-text rendering, interesting-link extraction and link metadata
//! are owned by the email pipeline (WP02, `omni-email`), which this crate may
//! not depend on. They enter through [`BodyEnricher`], which the binary wires
//! to omni-email's implementations.

use std::sync::Arc;

use omni_core::email::{EmailAttachment, EmailLinkMetadata, EmailOrigin, FetchedEmail};

use crate::attachments::{attachment_part_id, encode_stable_attachment_id};
use crate::mime::{HeaderLine, ParsedMail};

const BLOB_PREFIX: &str = "imap";

/// Where a message physically lives right now (UIDs are per folder).
pub type MessageCoords = EmailOrigin;

/// Inputs for link metadata (`extractEmailLinkMetadata(parsed)`).
#[derive(Clone, Copy, Debug)]
pub struct LinkMetadataInput<'a> {
    pub html: Option<&'a str>,
    pub text: Option<&'a str>,
    /// Root header lines: lowercased key and the raw (folded) line.
    pub header_lines: &'a [HeaderLine],
}

/// Body enrichment owned by the email pipeline (`htmlToText.ts`, `linkMetadata.ts`).
pub trait BodyEnricher: Send + Sync {
    /// `htmlToText(html)`.
    fn html_to_text(&self, html: &str) -> String;
    /// `extractInterestingLinks(html)`.
    fn interesting_links(&self, html: &str) -> Vec<String>;
    /// `extractEmailLinkMetadata(parsed)`.
    fn link_metadata(&self, input: LinkMetadataInput<'_>) -> EmailLinkMetadata;
}

pub type SharedEnricher = Arc<dyn BodyEnricher>;

/// `imap|<folder>|<uidValidity>|<uid>|<index>`.
pub fn encode_attachment_blob_id(coords: &MessageCoords, index: usize) -> String {
    format!(
        "{BLOB_PREFIX}|{}|{}|{}|{index}",
        coords.folder, coords.uid_validity, coords.uid
    )
}

/// The attachment handle's coordinates and index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttachmentTarget {
    pub coords: MessageCoords,
    pub index: usize,
}

/// JS `Number(s)` restricted to the integers `Number.isInteger` accepts.
fn js_integer(s: &str) -> Option<f64> {
    let n = omni_core::js::string_to_number(s);
    (n.is_finite() && n.fract() == 0.0).then_some(n)
}

pub fn decode_attachment_blob_id(blob_id: &str) -> Option<AttachmentTarget> {
    let parts: Vec<&str> = blob_id.split('|').collect();
    if parts.len() != 5 || parts[0] != BLOB_PREFIX {
        return None;
    }
    let uid = js_integer(parts[3])?;
    let index = js_integer(parts[4])?;
    if !(0.0..=f64::from(u32::MAX)).contains(&uid) || !(0.0..=1e9).contains(&index) {
        return None;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Some(AttachmentTarget {
        coords: MessageCoords {
            folder: parts[1].to_owned(),
            uid_validity: parts[2].to_owned(),
            uid: uid as u32,
        },
        index: index as usize,
    })
}

/// Fallback ids carry folder coordinates (`imap|<folder>|<uv>|<uid>`).
pub fn decode_message_id(id: &str) -> Option<MessageCoords> {
    let parts: Vec<&str> = id.split('|').collect();
    if parts.len() != 4 || parts[0] != BLOB_PREFIX {
        return None;
    }
    let uid = js_integer(parts[3])?;
    if !(0.0..=f64::from(u32::MAX)).contains(&uid) {
        return None;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Some(MessageCoords {
        folder: parts[1].to_owned(),
        uid_validity: parts[2].to_owned(),
        uid: uid as u32,
    })
}

/// `mapParsedMessage`. The id is the RFC Message-ID when present (stable
/// across folder moves), else the folder coordinates.
pub fn map_parsed_message(
    parsed: &ParsedMail,
    coords: &MessageCoords,
    internal_date_ms: Option<i64>,
    enricher: &dyn BodyEnricher,
) -> FetchedEmail {
    let html = parsed.html.as_deref();
    let text_body = match html {
        Some(html) => enricher.html_to_text(html),
        None => parsed.text.clone().unwrap_or_default(),
    };
    // `from?.address ?? from?.name ?? ""`: an empty address string is not nullish.
    let from = parsed
        .from
        .as_ref()
        .and_then(|list| list.first())
        .map(|a| a.address.clone().unwrap_or_else(|| a.name.clone()))
        .unwrap_or_default();

    let attachments = parsed
        .attachments
        .iter()
        .enumerate()
        .map(|(index, attachment)| {
            let part_id = attachment_part_id(attachment);
            EmailAttachment {
                blob_id: encode_attachment_blob_id(coords, index),
                attachment_id: match (&parsed.message_id, &part_id) {
                    (Some(message_id), Some(part)) => {
                        Some(encode_stable_attachment_id(message_id, part))
                    }
                    _ => None,
                },
                part_id,
                disposition: attachment.content_disposition.clone(),
                content_id: attachment.content_id.clone(),
                name: attachment
                    .filename
                    .clone()
                    .unwrap_or_else(|| "unnamed".to_owned()),
                mime_type: attachment.content_type.clone(),
                size: attachment.size() as u64,
            }
        })
        .collect();

    let received_ms = internal_date_ms.or(parsed.date);
    FetchedEmail {
        id: parsed.message_id.clone().unwrap_or_else(|| {
            format!(
                "{BLOB_PREFIX}|{}|{}|{}",
                coords.folder, coords.uid_validity, coords.uid
            )
        }),
        origin: Some(coords.clone()),
        subject: parsed.subject.clone().unwrap_or_default(),
        from,
        to: Some(ParsedMail::flat_addresses(parsed.to.as_ref())),
        cc: Some(ParsedMail::flat_addresses(parsed.cc.as_ref())),
        reply_to: Some(
            parsed
                .reply_to
                .as_ref()
                .map(|list| {
                    list.iter()
                        .filter_map(|a| a.address.clone().filter(|s| !s.is_empty()))
                        .collect()
                })
                .unwrap_or_default(),
        ),
        message_id: parsed.message_id.clone(),
        in_reply_to: parsed.in_reply_to.clone(),
        references: Some(parsed.references.clone().unwrap_or_default()),
        text_body,
        links: html
            .map(|h| enricher.interesting_links(h))
            .unwrap_or_default(),
        link_metadata: Some(enricher.link_metadata(LinkMetadataInput {
            html,
            text: parsed.text.as_deref(),
            header_lines: &parsed.header_lines,
        })),
        received_at: received_ms
            .map(omni_core::js::to_iso_string)
            .unwrap_or_default(),
        attachments,
    }
}

/// `Buffer.byteLength(JSON.stringify(value))` for cache accounting.
pub(crate) fn estimate_bytes<T: serde::Serialize>(value: &T) -> usize {
    serde_json::to_vec(value)
        .map(|v| v.len())
        .unwrap_or(usize::MAX)
}
