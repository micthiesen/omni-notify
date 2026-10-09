//! Durable per-issue repair reservations.
//!
//! `observer-repair-state` is keyed by the numeric `issueId`. A reservation is
//! taken before any mutation; an execution found interrupted is converted to a
//! needs-attention outcome rather than repeated.

use omni_store::cbor::{self, Extra, JsValue};
use omni_store::entity::{self, Entity};
use omni_store::{DocMeta, DocOps as _, DocWrite as _, Store, StoreError, Tx};
use serde::{Deserialize, Serialize};

pub const OBSERVER_REPAIR_LEASE_MS: i64 = 30 * 60 * 1_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RepairPhase {
    Reserved,
    Executing,
    Repaired,
    Unhandled,
    Commented,
    Resolved,
    Done,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RepairOutcome {
    Repaired,
    Unhandled,
}

impl RepairOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            RepairOutcome::Repaired => "repaired",
            RepairOutcome::Unhandled => "unhandled",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeliveryState {
    Pending,
    Sending,
    Sent,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairLease {
    pub owner: String,
    pub expires_at: i64,
}

/// `ObserverRepairState`, the `observer-repair-state` document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObserverRepairState {
    pub issue_id: i64,
    pub revision: String,
    pub phase: RepairPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<RepairOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub notification: DeliveryState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease: Option<RepairLease>,
    /// Unknown top-level fields, written back unchanged.
    #[serde(flatten)]
    pub extra: Extra,
}

impl ObserverRepairState {
    fn reserved(issue_id: i64, revision: &str) -> Self {
        Self {
            issue_id,
            revision: revision.to_owned(),
            phase: RepairPhase::Reserved,
            outcome: None,
            message: None,
            notification: DeliveryState::Pending,
            lease: None,
            extra: Extra::new(),
        }
    }
}

impl Entity for ObserverRepairState {
    const NAME: &'static str = "observer-repair-state";
    type Key = i64;
    fn key(&self) -> i64 {
        self.issue_id
    }
}

/// `"<operation>: <cause>"`.
#[derive(Debug, thiserror::Error)]
#[error("{operation}: {cause}")]
pub struct RepairPersistenceError {
    pub operation: String,
    #[source]
    pub cause: PersistenceCause,
}

#[derive(Debug, thiserror::Error)]
pub enum PersistenceCause {
    #[error("{0}")]
    Store(#[from] StoreError),
    #[error("{0}")]
    Invalid(String),
}

fn fail(operation: String) -> impl FnOnce(PersistenceCause) -> RepairPersistenceError {
    move |cause| RepairPersistenceError { operation, cause }
}

fn pk(issue_id: i64) -> Result<String, StoreError> {
    entity::pk::<ObserverRepairState>(&issue_id)
}

fn decode(pk: &str, value: JsValue) -> Result<ObserverRepairState, PersistenceCause> {
    cbor::from_value(value)
        .map_err(|e| PersistenceCause::Invalid(format!("invalid stored repair state {pk}: {e}")))
}

fn read(tx: &Tx<'_>, pk: &str) -> Result<Option<ObserverRepairState>, PersistenceCause> {
    match tx.get_raw_row(pk)? {
        Some(raw) => decode(pk, raw.decode()?).map(Some),
        None => Ok(None),
    }
}

fn write(
    tx: &mut Tx<'_>,
    pk: &str,
    state: &ObserverRepairState,
    now: i64,
) -> Result<(), PersistenceCause> {
    let value = cbor::to_value(state).map_err(|source| StoreError::Encode {
        pk: pk.to_owned(),
        source,
    })?;
    tx.upsert_doc(
        pk,
        &value,
        DocMeta {
            entity: Some(ObserverRepairState::NAME.to_owned()),
            version: 0,
            expires_at: None,
            updated_at: Some(now),
        },
    )?;
    Ok(())
}

/// Reserves `issue_id` at `revision` for `owner`, or `None` when the revision is
/// already done, another revision is still in progress, or a live lease exists.
pub async fn acquire_issue(
    store: &Store,
    issue_id: i64,
    revision: &str,
    owner: &str,
    now: i64,
) -> Result<Option<ObserverRepairState>, RepairPersistenceError> {
    let revision = revision.to_owned();
    let owner = owner.to_owned();
    store
        .write(move |tx| -> Result<_, PersistenceCause> {
            let pk = pk(issue_id)?;
            let current = read(tx, &pk)?;
            let next = match current {
                Some(current) if current.phase == RepairPhase::Done => {
                    if current.revision == revision {
                        return Ok(None);
                    }
                    ObserverRepairState::reserved(issue_id, &revision)
                }
                Some(current) => {
                    if current.revision != revision
                        || current
                            .lease
                            .as_ref()
                            .is_some_and(|lease| lease.expires_at > now)
                    {
                        return Ok(None);
                    }
                    let mut next = current;
                    if next.phase == RepairPhase::Executing {
                        next.phase = RepairPhase::Unhandled;
                        next.outcome = Some(RepairOutcome::Unhandled);
                        next.message = Some(
                            "Repair interrupted while executing; manual handling required"
                                .to_owned(),
                        );
                    }
                    next
                }
                None => ObserverRepairState::reserved(issue_id, &revision),
            };
            let mut leased = next;
            leased.lease = Some(RepairLease {
                owner,
                expires_at: now + OBSERVER_REPAIR_LEASE_MS,
            });
            write(tx, &pk, &leased, now)?;
            Ok(Some(leased))
        })
        .await
        .map_err(fail(format!("acquire Observer issue {issue_id}")))
}

/// Saves `state` if `owner` still holds the unexpired lease (which is kept).
pub async fn save_issue(
    store: &Store,
    state: &ObserverRepairState,
    owner: &str,
    now: i64,
) -> Result<(), RepairPersistenceError> {
    let issue_id = state.issue_id;
    let mut next = state.clone();
    let owner = owner.to_owned();
    store
        .write(move |tx| -> Result<_, PersistenceCause> {
            let pk = pk(issue_id)?;
            let Some(current) = read(tx, &pk)? else {
                return Err(PersistenceCause::Invalid(format!(
                    "No Observer repair state exists for {issue_id}"
                )));
            };
            let lease = match current.lease {
                Some(lease) if lease.owner == owner => lease,
                _ => {
                    return Err(PersistenceCause::Invalid(format!(
                        "Observer repair lease is not owned by {owner}"
                    )));
                }
            };
            if lease.expires_at <= now {
                return Err(PersistenceCause::Invalid(format!(
                    "Observer repair lease owned by {owner} has expired"
                )));
            }
            next.lease = Some(lease);
            write(tx, &pk, &next, now)
        })
        .await
        .map_err(fail(format!("save Observer issue {issue_id}")))
}

/// Drops `owner`'s lease; another owner's lease is left alone.
pub async fn release_issue(
    store: &Store,
    issue_id: i64,
    owner: &str,
    now: i64,
) -> Result<(), RepairPersistenceError> {
    let owner = owner.to_owned();
    store
        .write(move |tx| -> Result<_, PersistenceCause> {
            let pk = pk(issue_id)?;
            let Some(mut current) = read(tx, &pk)? else {
                return Ok(());
            };
            if current.lease.as_ref().map(|l| l.owner.as_str()) != Some(owner.as_str()) {
                return Ok(());
            }
            current.lease = None;
            write(tx, &pk, &current, now)
        })
        .await
        .map_err(fail(format!("release Observer issue {issue_id}")))
}

/// Up to 100 unfinished states. Fails closed when any stored state is invalid.
pub async fn list_pending(
    store: &Store,
) -> Result<Vec<ObserverRepairState>, RepairPersistenceError> {
    store
        .read(|docs| -> Result<_, StoreError> {
            docs.get_docs_by_entity(ObserverRepairState::NAME)
        })
        .await
        .map_err(PersistenceCause::Store)
        .and_then(|rows| {
            rows.into_iter()
                .map(|(pk, value)| decode(&pk, value))
                .collect::<Result<Vec<_>, _>>()
        })
        .map(|states| {
            states
                .into_iter()
                .filter(|state| state.phase != RepairPhase::Done)
                .take(100)
                .collect()
        })
        .map_err(fail("list pending Observer repairs".to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_explicit_undefined_and_keeps_unknown_fields() {
        let mut stored = cbor::to_value(&ObserverRepairState::reserved(7, "r")).unwrap();
        let object = stored.as_object_mut().unwrap();
        object.insert("outcome".to_owned(), JsValue::Undefined);
        object.insert("message".to_owned(), JsValue::Undefined);
        object.insert("lease".to_owned(), JsValue::Undefined);
        object.insert("future".to_owned(), JsValue::Int(1));
        let state: ObserverRepairState = cbor::from_value(stored).unwrap();
        assert_eq!((state.outcome, state.message.as_deref()), (None, None));
        assert_eq!(state.lease, None);
        assert_eq!(
            state.extra,
            Extra::from([("future".to_owned(), JsValue::Int(1))])
        );
        let JsValue::Object(back) = cbor::to_value(&state).unwrap() else {
            panic!("object");
        };
        let keys: Vec<&str> = back.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            ["issueId", "revision", "phase", "notification", "future"]
        );
    }
}
