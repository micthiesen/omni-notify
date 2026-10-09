//! Last-dispatch watermark (`src/email/persistence.ts`) for the watchdog and
//! IMAP UIDVALIDITY recovery. Its own row, so cursor saves and dispatch marks
//! never clobber each other; the historical `jmap-email-dispatch` name is kept.

use omni_store::cbor::Extra;
use omni_store::entity::{Entity, EntityOps as _, EntityWrite as _, UpsertOpts};
use omni_store::{Store, StoreError};
use serde::{Deserialize, Serialize};

pub const SINGLETON: &str = "singleton";

/// `EmailDispatchData` (entity `jmap-email-dispatch`, key `key`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailDispatchData {
    /// Always `"singleton"`.
    pub key: String,
    /// Epoch ms of the last batch dispatched to handlers.
    pub last_dispatched_at: i64,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity for EmailDispatchData {
    const NAME: &'static str = "jmap-email-dispatch";
    type Key = String;
    fn key(&self) -> String {
        self.key.clone()
    }
    fn validate(&self) -> Result<(), String> {
        if self.key == SINGLETON {
            Ok(())
        } else {
            Err(format!("key must be \"{SINGLETON}\""))
        }
    }
}

pub async fn last_dispatched_at(store: &Store) -> Result<Option<i64>, StoreError> {
    store
        .read(|docs| docs.get::<EmailDispatchData>(&SINGLETON.to_owned()))
        .await
        .map(|row| row.map(|row| row.last_dispatched_at))
}

pub async fn save_last_dispatched_at(store: &Store, timestamp: i64) -> Result<(), StoreError> {
    let row = EmailDispatchData {
        key: SINGLETON.to_owned(),
        last_dispatched_at: timestamp,
        extra: Extra::new(),
    };
    store
        .write(move |tx| tx.upsert(&row, UpsertOpts::default()))
        .await
}
