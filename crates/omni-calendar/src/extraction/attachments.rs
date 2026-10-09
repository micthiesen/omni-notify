//! Supported attachment download.

use futures::StreamExt as _;
use omni_core::email::EmailAttachment;

use crate::support::{AttachmentSource, DownloadedAttachment};

const LOG: &str = "Main:CalendarEvents";
const ALLOWED_MIME_TYPES: [&str; 1] = ["application/pdf"];
/// 5 MB.
pub const MAX_ATTACHMENT_SIZE: u64 = 5 * 1024 * 1024;

#[allow(clippy::cast_precision_loss)]
fn megabytes(bytes: u64) -> f64 {
    bytes as f64 / 1024.0 / 1024.0
}

/// Downloads PDF attachments of at most 5 MB, three at a time. Unsupported or
/// oversized attachments are skipped; a failed download is warned about and
/// skipped. Results keep the attachment order.
pub async fn download_supported_attachments(
    source: &dyn AttachmentSource,
    attachments: &[EmailAttachment],
) -> Vec<DownloadedAttachment> {
    let mut supported = Vec::new();
    for attachment in attachments {
        if !ALLOWED_MIME_TYPES.contains(&attachment.mime_type.as_str()) {
            tracing::debug!(
                target: LOG,
                "Skipping attachment \"{}\" (unsupported type: {})",
                attachment.name,
                attachment.mime_type
            );
        } else if attachment.size > MAX_ATTACHMENT_SIZE {
            tracing::debug!(
                target: LOG,
                "Skipping attachment \"{}\" (too large: {}MB)",
                attachment.name,
                omni_core::js::to_fixed(megabytes(attachment.size), 1)
            );
        } else {
            supported.push(attachment);
        }
    }
    let downloads: Vec<_> = supported
        .into_iter()
        .map(|attachment| download_one(source, attachment))
        .collect();
    let results: Vec<Option<DownloadedAttachment>> =
        futures::stream::iter(downloads).buffered(3).collect().await;
    results.into_iter().flatten().collect()
}

async fn download_one(
    source: &dyn AttachmentSource,
    attachment: &EmailAttachment,
) -> Option<DownloadedAttachment> {
    match source.download(attachment).await {
        Ok(Some(downloaded)) => {
            #[allow(clippy::cast_precision_loss)]
            let kb = downloaded.data.len() as f64 / 1024.0;
            tracing::debug!(
                target: LOG,
                "Downloaded attachment \"{}\" ({}, {}KB)",
                downloaded.name,
                downloaded.mime_type,
                omni_core::js::to_fixed(kb, 0)
            );
            Some(downloaded)
        }
        Ok(None) => None,
        Err(error) => {
            tracing::warn!(
                target: LOG,
                "Failed to download attachment \"{}\": {error}",
                attachment.name
            );
            None
        }
    }
}
