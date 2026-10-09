//! Durable recovery state with a per-service lease (`src/arr-recovery/persistence.ts`).
//!
//! `arr-recovery-state` is keyed by `kind`. Every save re-checks that the
//! caller still owns an unexpired lease, inside the same transaction.

use indexmap::IndexMap;
use omni_store::cbor::{self, JsValue};
use omni_store::entity::{self, Entity};
use omni_store::{DocMeta, DocOps as _, DocWrite as _, Store, StoreError, Tx};
use serde::{Deserialize, Serialize};

use super::types::{ArrCause, ArrKind, ArrRecoveryError, Decision, ImportFile, Target};
use crate::stored::{Stored, StoredFields};

/// A per-service lease outlives the 20-minute bounded run.
pub const RECOVERY_LEASE_MS: i64 = 25 * 60 * 1_000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Observation {
    pub fingerprint: String,
    pub first_seen_at: i64,
    pub last_seen_at: i64,
    pub observations: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_assessed_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActionPhase {
    Reserved,
    Submitted,
    Removed,
    Searching,
    Done,
    Uncertain,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NotificationState {
    Pending,
    Sending,
    Sent,
}

/// One reserved recovery mutation and its verification progress.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryAction {
    pub download_id: String,
    pub title: String,
    pub target: Target,
    pub files: Vec<ImportFile>,
    pub output_path: String,
    pub decision: Decision,
    pub phase: ActionPhase,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub notification: NotificationState,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Lease {
    pub owner: String,
    pub expires_at: i64,
}

/// `RecoveryState`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecoveryState {
    pub kind: ArrKind,
    pub observations: IndexMap<String, Observation>,
    pub actions: Vec<RecoveryAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease: Option<Lease>,
}

impl RecoveryState {
    pub fn empty(kind: ArrKind) -> Self {
        Self {
            kind,
            observations: IndexMap::new(),
            actions: Vec::new(),
            lease: None,
        }
    }
}

impl StoredFields for RecoveryState {
    const FIELDS: &'static [&'static str] = &["kind", "observations", "actions", "lease"];
}

/// The `arr-recovery-state` document (unknown top-level fields preserved).
pub type StoredRecoveryState = Stored<RecoveryState>;

impl Entity for Stored<RecoveryState> {
    const NAME: &'static str = "arr-recovery-state";
    type Key = String;
    fn key(&self) -> String {
        self.value.kind.as_str().to_owned()
    }
}

#[derive(Debug, thiserror::Error)]
enum TxError {
    #[error("{0}")]
    Store(#[from] StoreError),
    #[error("{0}")]
    Invalid(String),
}

fn fail(operation: String, error: TxError) -> ArrRecoveryError {
    match error {
        TxError::Store(store) => ArrRecoveryError::new(operation, ArrCause::Store(store)),
        TxError::Invalid(message) => ArrRecoveryError::message(operation, message),
    }
}

fn pk(kind: ArrKind) -> Result<String, StoreError> {
    entity::pk::<StoredRecoveryState>(&kind.as_str().to_owned())
}

fn read(tx: &Tx<'_>, pk: &str, kind: ArrKind) -> Result<Option<StoredRecoveryState>, TxError> {
    let Some(raw) = tx.get_raw_row(pk)? else {
        return Ok(None);
    };
    let state: StoredRecoveryState = cbor::from_value(raw.decode()?)
        .map_err(|e| TxError::Invalid(format!("invalid stored recovery state: {e}")))?;
    if state.value.kind != kind {
        return Err(TxError::Invalid(format!(
            "Arr recovery state kind mismatch for {kind}"
        )));
    }
    Ok(Some(state))
}

fn write(tx: &mut Tx<'_>, pk: &str, state: &StoredRecoveryState, now: i64) -> Result<(), TxError> {
    let value: JsValue = cbor::to_value(state).map_err(|e| {
        TxError::Store(StoreError::Encode {
            pk: pk.to_owned(),
            source: e,
        })
    })?;
    tx.upsert_doc(
        pk,
        &value,
        DocMeta {
            entity: Some(StoredRecoveryState::NAME.to_owned()),
            version: 0,
            expires_at: None,
            updated_at: Some(now),
        },
    )?;
    Ok(())
}

/// Takes the lease for `kind` unless another owner holds an unexpired one.
pub async fn acquire_state(
    store: &Store,
    kind: ArrKind,
    owner: &str,
    now: i64,
) -> Result<Option<StoredRecoveryState>, ArrRecoveryError> {
    let owner = owner.to_owned();
    store
        .write(move |tx| -> Result<_, TxError> {
            let pk = pk(kind)?;
            let current =
                read(tx, &pk, kind)?.unwrap_or_else(|| Stored::new(RecoveryState::empty(kind)));
            if current
                .value
                .lease
                .as_ref()
                .is_some_and(|lease| lease.expires_at > now)
            {
                return Ok(None);
            }
            let mut next = current;
            next.value.lease = Some(Lease {
                owner,
                expires_at: now + RECOVERY_LEASE_MS,
            });
            write(tx, &pk, &next, now)?;
            Ok(Some(next))
        })
        .await
        .map_err(|e| fail(format!("acquire {kind} recovery state"), e))
}

/// Saves `state` if `owner` still holds the unexpired lease (which is kept).
pub async fn save_state(
    store: &Store,
    state: &StoredRecoveryState,
    owner: &str,
    now: i64,
) -> Result<(), ArrRecoveryError> {
    let kind = state.value.kind;
    let mut next = state.clone();
    let owner = owner.to_owned();
    store
        .write(move |tx| -> Result<_, TxError> {
            let pk = pk(kind)?;
            let Some(current) = read(tx, &pk, kind)? else {
                return Err(TxError::Invalid(format!(
                    "No recovery state exists for {kind}"
                )));
            };
            let lease = match current.value.lease {
                Some(lease) if lease.owner == owner => lease,
                _ => {
                    return Err(TxError::Invalid(format!(
                        "Arr recovery state lease is not owned by {owner}"
                    )));
                }
            };
            if lease.expires_at <= now {
                return Err(TxError::Invalid(format!(
                    "Arr recovery state lease owned by {owner} has expired"
                )));
            }
            next.value.lease = Some(lease);
            write(tx, &pk, &next, now)
        })
        .await
        .map_err(|e| fail(format!("save {kind} recovery state"), e))
}

/// Drops `owner`'s lease; another owner's lease is left alone.
pub async fn release_state(
    store: &Store,
    kind: ArrKind,
    owner: &str,
    now: i64,
) -> Result<(), ArrRecoveryError> {
    let owner = owner.to_owned();
    store
        .write(move |tx| -> Result<_, TxError> {
            let pk = pk(kind)?;
            let Some(mut current) = read(tx, &pk, kind)? else {
                return Ok(());
            };
            if current.value.lease.as_ref().map(|l| l.owner.as_str()) != Some(owner.as_str()) {
                return Ok(());
            }
            current.value.lease = None;
            write(tx, &pk, &current, now)
        })
        .await
        .map_err(|e| fail(format!("release {kind} recovery state"), e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_list_every_serialized_property() {
        let state = RecoveryState {
            lease: Some(Lease {
                owner: "o".into(),
                expires_at: 1,
            }),
            ..RecoveryState::empty(ArrKind::Sonarr)
        };
        let value = cbor::to_value(&state).unwrap();
        let keys: Vec<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, RecoveryState::FIELDS);
    }
}
