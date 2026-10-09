//! Durable archive receipts (`src/email/archive/persistence.ts`).
//!
//! Raw keys (not entities): `email-archive:action:<sha256hex(idempotencyKey)>`
//! (entity column `email-archive-action`), the latest-reservation marker
//! `email-archive:message:<sha256hex(messageId)>` and the reservation history
//! `email-archive:history:<sha256hex(messageId)>`.

use std::collections::HashSet;
use std::sync::LazyLock;

use omni_store::cbor::{self, Extra, JsValue};
use omni_store::{DocMeta, DocOps as _, DocWrite as _, Store, StoreError, Tx};
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::ops::archive::{ArchiveIdentity, ArchiveLocation, ArchiveSnapshot};
use crate::ops::auto_read::AutoReadProtection;

pub const ACTION_ENTITY: &str = "email-archive-action";
pub const MESSAGE_ENTITY: &str = "email-archive-message";
pub const HISTORY_ENTITY: &str = "email-archive-history";
pub const ACTION_PREFIX: &str = "email-archive:action:";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveActionStatus {
    Queued,
    Cancelled,
    Claimed,
    Archived,
    Uncertain,
    Failed,
    RestoreClaimed,
    Restored,
    RestoreUncertain,
    CopyClaimed,
    CopyVerified,
    DeleteClaimed,
    ExpungeClaimed,
    CopiedSourceRetained,
    RestoreCopyClaimed,
    RestoreCopyVerified,
    RestoreDeleteClaimed,
    RestoreExpungeClaimed,
    RestoreCopiedSourceRetained,
}

impl ArchiveActionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Cancelled => "cancelled",
            Self::Claimed => "claimed",
            Self::Archived => "archived",
            Self::Uncertain => "uncertain",
            Self::Failed => "failed",
            Self::RestoreClaimed => "restore_claimed",
            Self::Restored => "restored",
            Self::RestoreUncertain => "restore_uncertain",
            Self::CopyClaimed => "copy_claimed",
            Self::CopyVerified => "copy_verified",
            Self::DeleteClaimed => "delete_claimed",
            Self::ExpungeClaimed => "expunge_claimed",
            Self::CopiedSourceRetained => "copied_source_retained",
            Self::RestoreCopyClaimed => "restore_copy_claimed",
            Self::RestoreCopyVerified => "restore_copy_verified",
            Self::RestoreDeleteClaimed => "restore_delete_claimed",
            Self::RestoreExpungeClaimed => "restore_expunge_claimed",
            Self::RestoreCopiedSourceRetained => "restore_copied_source_retained",
        }
    }

    /// No mailbox operation is pending or unproven.
    pub fn is_settled(self) -> bool {
        matches!(
            self,
            Self::Archived | Self::Restored | Self::Cancelled | Self::Failed
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveReason {
    TransportUnavailable,
    SourceUnavailable,
    NativeMoveUnavailable,
    SafeMoveUnavailable,
    VerificationFailed,
    Uncertain,
    CopyUncertain,
    CopiedSourceRetained,
    CopiedSourceDeleted,
    SourceMarkUncertain,
    SourceExpungeUncertain,
}

/// `ArchiveAction`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveAction {
    pub action_id: String,
    pub identity: ArchiveIdentity,
    pub status: ArchiveActionStatus,
    pub attempts: i64,
    pub next_attempt_at: i64,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "cbor::undefined_as_none"
    )]
    pub snapshot: Option<ArchiveSnapshot>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "cbor::undefined_as_none"
    )]
    pub restore_snapshot: Option<ArchiveSnapshot>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "cbor::undefined_as_none"
    )]
    pub destination: Option<ArchiveLocation>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "cbor::undefined_as_none"
    )]
    pub restored_location: Option<ArchiveLocation>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "cbor::undefined_as_none"
    )]
    pub reason: Option<ArchiveReason>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// A `Partial<ArchiveAction>` patch; `reason: Some(None)` clears the reason.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ArchivePatch {
    pub snapshot: Option<ArchiveSnapshot>,
    pub restore_snapshot: Option<ArchiveSnapshot>,
    pub destination: Option<ArchiveLocation>,
    pub restored_location: Option<ArchiveLocation>,
    pub attempts: Option<i64>,
    pub next_attempt_at: Option<i64>,
    pub reason: Option<Option<ArchiveReason>>,
}

impl ArchivePatch {
    pub fn reason(reason: ArchiveReason) -> Self {
        Self {
            reason: Some(Some(reason)),
            ..Self::default()
        }
    }

    pub fn clear_reason() -> Self {
        Self {
            reason: Some(None),
            ..Self::default()
        }
    }

    pub fn next_attempt_at(at: i64) -> Self {
        Self {
            next_attempt_at: Some(at),
            ..Self::default()
        }
    }

    fn apply(self, action: &mut ArchiveAction) {
        if let Some(v) = self.snapshot {
            action.snapshot = Some(v);
        }
        if let Some(v) = self.restore_snapshot {
            action.restore_snapshot = Some(v);
        }
        if let Some(v) = self.destination {
            action.destination = Some(v);
        }
        if let Some(v) = self.restored_location {
            action.restored_location = Some(v);
        }
        if let Some(v) = self.attempts {
            action.attempts = v;
        }
        if let Some(v) = self.next_attempt_at {
            action.next_attempt_at = v;
        }
        if let Some(v) = self.reason {
            action.reason = v;
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ArchiveActionError {
    /// A rejected request (`ArchiveActionError({ message })`).
    #[error("{0}")]
    Rejected(String),
    /// A mailbox read that the workflow does not absorb into a receipt state.
    #[error(transparent)]
    Imap(#[from] crate::protocol::ImapError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// The history row key for a Message-ID (exposed for legacy-row tests).
pub fn history_key_for(message_id: &str) -> String {
    history_key(message_id)
}

fn sha256_hex(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

pub fn action_key(action_id: &str) -> String {
    format!("{ACTION_PREFIX}{action_id}")
}

fn marker_key(message_id: &str) -> String {
    format!("email-archive:message:{}", sha256_hex(message_id))
}

fn history_key(message_id: &str) -> String {
    format!("email-archive:history:{}", sha256_hex(message_id))
}

/// `archiveActionId`: sha256 hex of the idempotency key.
pub fn archive_action_id(idempotency_key: &str) -> String {
    sha256_hex(idempotency_key)
}

fn corrupt(pk: &str, reason: impl std::fmt::Display) -> StoreError {
    StoreError::CorruptRow {
        pk: pk.to_owned(),
        reason: reason.to_string(),
    }
}

pub fn decode_action(pk: &str, value: JsValue) -> Result<ArchiveAction, StoreError> {
    cbor::from_value(value).map_err(|e| corrupt(pk, e))
}

fn encode<T: Serialize>(pk: &str, value: &T) -> Result<JsValue, StoreError> {
    cbor::to_value(value).map_err(|source| StoreError::Encode {
        pk: pk.to_owned(),
        source,
    })
}

fn meta(entity: &str) -> DocMeta {
    DocMeta {
        entity: Some(entity.to_owned()),
        ..DocMeta::default()
    }
}

fn read_action<D: omni_store::DocOps + ?Sized>(
    docs: &D,
    action_id: &str,
) -> Result<Option<ArchiveAction>, StoreError> {
    let pk = action_key(action_id);
    match docs.get_raw_row(&pk)? {
        Some(row) => decode_action(&pk, row.decode()?).map(Some),
        None => Ok(None),
    }
}

/// Every action ever reserved for a Message-ID (history plus the legacy
/// latest-action marker), de-duplicated in first-seen order.
fn reservation_ids<D: omni_store::DocOps + ?Sized>(
    docs: &D,
    message_id: &str,
) -> Result<Vec<String>, StoreError> {
    let mut ids: Vec<String> = Vec::new();
    let history_pk = history_key(message_id);
    if let Some(row) = docs.get_raw_row(&history_pk)? {
        let values: Vec<String> =
            cbor::from_value(row.decode()?).map_err(|e| corrupt(&history_pk, e))?;
        for id in values {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
    }
    let marker_pk = marker_key(message_id);
    if let Some(row) = docs.get_raw_row(&marker_pk)? {
        let latest: String = cbor::from_value(row.decode()?).map_err(|e| corrupt(&marker_pk, e))?;
        if !ids.contains(&latest) {
            ids.push(latest);
        }
    }
    Ok(ids)
}

/// Any unsettled action for the same Message-ID blocks a sibling copy; the
/// same physical copy may be re-reserved only after restore/cancel/failure.
fn reservation_conflict(
    previous: &ArchiveAction,
    identity: &ArchiveIdentity,
) -> Option<&'static str> {
    if previous.identity.same_copy(identity) {
        return match previous.status {
            ArchiveActionStatus::Restored
            | ArchiveActionStatus::Cancelled
            | ArchiveActionStatus::Failed => None,
            _ => Some("Message already has an archive action"),
        };
    }
    if previous.status.is_settled() {
        None
    } else {
        Some("Another copy with this Message-ID has an unresolved archive action")
    }
}

static INBOX_MESSAGE_ID: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"^<[^<>\s\x00-\x1f\x7f]+>$").ok());

fn write_action(tx: &mut Tx<'_>, action: &ArchiveAction) -> Result<(), StoreError> {
    let pk = action_key(&action.action_id);
    let value = encode(&pk, action)?;
    tx.upsert_doc(&pk, &value, meta(ACTION_ENTITY))
}

/// Reserves an exact Inbox copy under an idempotency key.
pub async fn queue_archive_action(
    store: &Store,
    idempotency_key: &str,
    identity: ArchiveIdentity,
) -> Result<ArchiveAction, ArchiveActionError> {
    let valid_id = INBOX_MESSAGE_ID
        .as_ref()
        .is_some_and(|re| re.is_match(&identity.message_id));
    if identity.folder != "INBOX" || identity.uid < 1 || !valid_id {
        return Err(ArchiveActionError::Rejected(
            "Invalid exact Inbox identity".to_owned(),
        ));
    }
    let action_id = archive_action_id(idempotency_key);
    store
        .write(move |tx| -> Result<ArchiveAction, ArchiveActionError> {
            let now = tx.now_ms();
            if let Some(existing) = read_action(tx, &action_id)? {
                if existing.identity != identity {
                    return Err(ArchiveActionError::Rejected(
                        "Idempotency key belongs to a different message".to_owned(),
                    ));
                }
                return Ok(existing);
            }
            let history = reservation_ids(tx, &identity.message_id)?;
            for previous_id in &history {
                let Some(previous) = read_action(tx, previous_id)? else {
                    continue;
                };
                if let Some(conflict) = reservation_conflict(&previous, &identity) {
                    return Err(ArchiveActionError::Rejected(conflict.to_owned()));
                }
            }
            let action = ArchiveAction {
                action_id: action_id.clone(),
                identity: identity.clone(),
                status: ArchiveActionStatus::Queued,
                attempts: 0,
                next_attempt_at: now,
                created_at: now,
                updated_at: now,
                snapshot: None,
                restore_snapshot: None,
                destination: None,
                restored_location: None,
                reason: None,
                extra: Extra::new(),
            };
            write_action(tx, &action)?;
            let marker_pk = marker_key(&identity.message_id);
            tx.upsert_doc(
                &marker_pk,
                &JsValue::String(action_id.clone()),
                meta(MESSAGE_ENTITY),
            )?;
            let mut ids = history;
            if !ids.contains(&action_id) {
                ids.push(action_id.clone());
            }
            let history_pk = history_key(&identity.message_id);
            tx.upsert_doc(
                &history_pk,
                &JsValue::Array(ids.into_iter().map(JsValue::String).collect()),
                meta(HISTORY_ENTITY),
            )?;
            Ok(action)
        })
        .await
}

pub async fn get_archive_action(
    store: &Store,
    action_id: &str,
) -> Result<Option<ArchiveAction>, StoreError> {
    let action_id = action_id.to_owned();
    store.read(move |docs| read_action(docs, &action_id)).await
}

pub async fn list_archive_actions(store: &Store) -> Result<Vec<ArchiveAction>, StoreError> {
    store
        .read(|docs| {
            docs.get_raw_rows_by_prefix(ACTION_PREFIX)?
                .into_iter()
                .map(|row| decode_action(&row.pk, row.decode()?))
                .collect()
        })
        .await
}

/// Compare-and-set status transition with a patch.
pub async fn update_archive_action(
    store: &Store,
    action_id: &str,
    expected: ArchiveActionStatus,
    status: ArchiveActionStatus,
    patch: ArchivePatch,
) -> Result<ArchiveAction, ArchiveActionError> {
    let action_id = action_id.to_owned();
    store
        .write(move |tx| -> Result<ArchiveAction, ArchiveActionError> {
            let now = tx.now_ms();
            let Some(mut action) = read_action(tx, &action_id)? else {
                return Err(ArchiveActionError::Rejected(
                    "Archive action not found".to_owned(),
                ));
            };
            if action.status != expected {
                return Err(ArchiveActionError::Rejected(format!(
                    "Archive action is {}, expected {}",
                    action.status.as_str(),
                    expected.as_str()
                )));
            }
            patch.apply(&mut action);
            action.status = status;
            action.updated_at = now;
            write_action(tx, &action)?;
            Ok(action)
        })
        .await
}

/// All actions recorded for a Message-ID, across every physical copy.
pub async fn list_archive_actions_for_message(
    store: &Store,
    message_id: &str,
) -> Result<Vec<ArchiveAction>, StoreError> {
    let message_id = message_id.to_owned();
    store
        .read(move |docs| {
            let mut actions = Vec::new();
            for id in reservation_ids(docs, &message_id)? {
                if let Some(action) = read_action(docs, &id)? {
                    actions.push(action);
                }
            }
            Ok(actions)
        })
        .await
}

fn same_location(left: Option<&ArchiveLocation>, right: Option<&ArchiveLocation>) -> bool {
    matches!((left, right), (Some(l), Some(r)) if l.same(r))
}

/// Suppresses only mailbox events an action's own move caused.
pub async fn is_archive_action_message(
    store: &Store,
    message_id: &str,
    origin: Option<&ArchiveLocation>,
) -> Result<bool, StoreError> {
    let Some(origin) = origin else {
        return Ok(false);
    };
    let actions = list_archive_actions_for_message(store, message_id).await?;
    // Inbox copies known to predate a restore: every action's source copy and
    // any completed restore. A restored copy gets a UID above them.
    let newest_known_inbox_uid = actions
        .iter()
        .flat_map(|action| {
            let restored = if action.status == ArchiveActionStatus::Restored {
                action.restored_location.clone()
            } else {
                None
            };
            [Some(action.identity.location()), restored]
        })
        .flatten()
        .filter(|l| l.folder == "INBOX" && l.uid_validity == origin.uid_validity)
        .map(|l| l.uid)
        .max()
        .unwrap_or(0);
    for action in &actions {
        if action.identity.location().same(origin) {
            continue;
        }
        if matches!(
            action.status,
            ArchiveActionStatus::Queued
                | ArchiveActionStatus::Cancelled
                | ArchiveActionStatus::Failed
        ) {
            continue;
        }
        if same_location(Some(origin), action.destination.as_ref())
            || same_location(Some(origin), action.restored_location.as_ref())
        {
            return Ok(true);
        }
        if matches!(
            action.status,
            ArchiveActionStatus::Claimed | ArchiveActionStatus::Uncertain
        ) && origin.folder != "INBOX"
        {
            return Ok(true);
        }
        if matches!(
            action.status,
            ArchiveActionStatus::RestoreClaimed | ArchiveActionStatus::RestoreUncertain
        ) {
            // Without a recorded restore UID, suppress only Inbox UIDs above every known copy.
            let preexisting_copy =
                action.restored_location.is_some() || origin.uid <= newest_known_inbox_uid;
            if origin.folder == "INBOX" && !preexisting_copy {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Keeps action-owned archived mail unread when the Archive sweep runs.
pub async fn archive_auto_read_protection(
    store: &Store,
    archive_folder: &str,
    uid_validity: Option<&str>,
) -> Result<AutoReadProtection, StoreError> {
    let actions = list_archive_actions(store).await?;
    let mut excluded = HashSet::new();
    let mut fallback: Vec<String> = Vec::new();
    for action in actions {
        use ArchiveActionStatus as S;
        match action.status {
            S::Claimed
            | S::Uncertain
            | S::CopyClaimed
            | S::CopyVerified
            | S::DeleteClaimed
            | S::ExpungeClaimed
            | S::CopiedSourceRetained
            | S::RestoreClaimed
            | S::RestoreUncertain
            | S::RestoreCopyClaimed
            | S::RestoreCopyVerified
            | S::RestoreDeleteClaimed
            | S::RestoreExpungeClaimed
            | S::RestoreCopiedSourceRetained => {
                if !fallback.contains(&action.identity.message_id) {
                    fallback.push(action.identity.message_id.clone());
                }
                continue;
            }
            S::Archived => {}
            _ => continue,
        }
        match (&action.destination, uid_validity) {
            (Some(destination), Some(validity))
                if destination.folder == archive_folder && destination.uid_validity == validity =>
            {
                excluded.insert(destination.uid);
            }
            _ => {
                if !fallback.contains(&action.identity.message_id) {
                    fallback.push(action.identity.message_id.clone());
                }
            }
        }
    }
    Ok(AutoReadProtection {
        skip: false,
        excluded_uids: excluded,
        fallback_message_ids: fallback,
    })
}
