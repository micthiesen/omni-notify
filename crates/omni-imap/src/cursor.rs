//! Per-folder IMAP delta cursors and the
//! dispatch watermark read used by UIDVALIDITY recovery.

use omni_store::cbor::Extra;
use omni_store::entity::{Entity, EntityOps as _, EntityWrite as _, UpsertOpts};
use omni_store::{DocOps as _, Store, StoreError};
use serde::{Deserialize, Serialize};

use crate::sync::FolderState;

/// `imap-folder-cursor`, keyed by folder.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImapFolderCursor {
    pub folder: String,
    /// UIDVALIDITY when the cursor was written (decimal string).
    pub uid_validity: String,
    /// Next unseen UID: everything below it was dispatched or skipped.
    pub uid_next: u32,
    pub updated_at: i64,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity for ImapFolderCursor {
    const NAME: &'static str = "imap-folder-cursor";
    type Key = String;
    fn key(&self) -> String {
        self.folder.clone()
    }
}

pub async fn get_folder_cursor(
    store: &Store,
    folder: &str,
) -> Result<Option<FolderState>, StoreError> {
    let folder = folder.to_owned();
    let cursor = store
        .read(move |docs| docs.get::<ImapFolderCursor>(&folder))
        .await?;
    Ok(cursor.map(|c| FolderState {
        uid_validity: c.uid_validity,
        uid_next: c.uid_next,
    }))
}

pub async fn save_folder_cursor(
    store: &Store,
    folder: &str,
    uid_validity: &str,
    uid_next: u32,
) -> Result<(), StoreError> {
    let cursor = ImapFolderCursor {
        folder: folder.to_owned(),
        uid_validity: uid_validity.to_owned(),
        uid_next,
        updated_at: store.clock().now_ms(),
        extra: Extra::new(),
    };
    store
        .write(move |tx| tx.upsert(&cursor, UpsertOpts::default()))
        .await
}

/// The email pipeline's dispatch watermark (`jmap-email-dispatch` singleton,
/// owned by omni-email). Read here only for UIDVALIDITY recovery; the row
/// layout is a persisted contract.
pub const DISPATCH_WATERMARK_PK: &str = "$jmap-email-dispatch#s9:singleton";

pub async fn last_dispatched_at(store: &Store) -> Result<Option<i64>, StoreError> {
    let doc = store
        .read(|docs| docs.get_doc(DISPATCH_WATERMARK_PK))
        .await?;
    let Some(doc) = doc else { return Ok(None) };
    let at = doc
        .get("lastDispatchedAt")
        .and_then(omni_store::JsValue::as_f64)
        .ok_or_else(|| StoreError::CorruptRow {
            pk: DISPATCH_WATERMARK_PK.to_owned(),
            reason: "lastDispatchedAt is not a number".to_owned(),
        })?;
    #[allow(clippy::cast_possible_truncation)]
    Ok(Some(at as i64))
}
