//! Control registrations (`persistence.ts`, entity `ios-control-registration`,
//! key `<deviceId>:<controlId>`).

use omni_api::ios::ApnsEnvironment;
use omni_store::cbor::{self, Extra};
use omni_store::entity::{Entity, EntityOps, EntityWrite, ModifyOpts, UpsertOpts, prefix};
use omni_store::{DocOps, Store, StoreError};
use serde::{Deserialize, Serialize};

/// One registered control of one device.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IosControlRegistration {
    pub registration_id: String,
    pub device_id: String,
    pub control_id: String,
    pub slot: u8,
    pub push_token: String,
    pub environment: ApnsEnvironment,
    /// Hash of the slot state last delivered to this token.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "cbor::undefined_as_none"
    )]
    pub last_delivered_hash: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity for IosControlRegistration {
    const NAME: &'static str = "ios-control-registration";
    type Key = String;

    fn key(&self) -> String {
        self.registration_id.clone()
    }
}

/// A control as the app registers it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlInput {
    pub control_id: String,
    pub slot: u8,
    pub push_token: String,
    pub environment: ApnsEnvironment,
}

/// A registration operation failed (`PersistenceError`).
#[derive(Debug, thiserror::Error)]
#[error("PersistenceError: {operation} failed: {source}")]
pub struct RegistrationStoreError {
    pub operation: &'static str,
    #[source]
    pub source: StoreError,
}

fn fail(operation: &'static str) -> impl Fn(StoreError) -> RegistrationStoreError {
    move |source| RegistrationStoreError { operation, source }
}

pub fn registration_id(device_id: &str, control_id: &str) -> String {
    format!("{device_id}:{control_id}")
}

pub async fn list_registrations(
    store: &Store,
) -> Result<Vec<IosControlRegistration>, RegistrationStoreError> {
    store
        .read(|docs| docs.get_all::<IosControlRegistration>())
        .await
        .map_err(fail("read iOS control registrations"))
}

/// Replaces a device's complete registration set in one transaction. The
/// delivered hash survives only when slot, token and environment are unchanged.
pub async fn replace_device_registrations(
    store: &Store,
    device_id: &str,
    controls: Vec<ControlInput>,
) -> Result<Vec<IosControlRegistration>, RegistrationStoreError> {
    let device_id = device_id.to_owned();
    store
        .write(move |tx| {
            let now = tx.now_ms();
            let prefix = prefix::<IosControlRegistration>(&[])?;
            let existing: Vec<IosControlRegistration> = tx
                .get_raw_rows_by_prefix(&prefix)?
                .into_iter()
                .map(|raw| {
                    let pk = raw.pk.clone();
                    cbor::from_value::<IosControlRegistration>(raw.decode()?)
                        .map_err(|source| StoreError::Decode { pk, source })
                })
                .collect::<Result<Vec<_>, StoreError>>()?
                .into_iter()
                .filter(|row| row.device_id == device_id)
                .collect();
            let incoming: Vec<String> = controls
                .iter()
                .map(|c| registration_id(&device_id, &c.control_id))
                .collect();
            for row in &existing {
                if !incoming.contains(&row.registration_id) {
                    tx.delete::<IosControlRegistration>(&row.registration_id)?;
                }
            }
            controls
                .into_iter()
                .map(|control| {
                    let id = registration_id(&device_id, &control.control_id);
                    let previous = tx.get::<IosControlRegistration>(&id)?;
                    let unchanged = previous.as_ref().is_some_and(|p| {
                        p.slot == control.slot
                            && p.push_token == control.push_token
                            && p.environment == control.environment
                    });
                    let row = IosControlRegistration {
                        registration_id: id,
                        device_id: device_id.clone(),
                        control_id: control.control_id,
                        slot: control.slot,
                        push_token: control.push_token,
                        environment: control.environment,
                        last_delivered_hash: if unchanged {
                            previous
                                .as_ref()
                                .and_then(|p| p.last_delivered_hash.clone())
                        } else {
                            None
                        },
                        created_at: previous.as_ref().map_or(now, |p| p.created_at),
                        updated_at: now,
                        extra: Extra::new(),
                    };
                    tx.upsert(&row, UpsertOpts::default())?;
                    Ok(row)
                })
                .collect::<Result<Vec<_>, StoreError>>()
        })
        .await
        .map_err(fail("replace iOS control registrations"))
}

/// Records delivery only while the token is still the one pushed to.
pub async fn mark_delivered(
    store: &Store,
    registration_id: &str,
    push_token: &str,
    hash: &str,
) -> Result<(), RegistrationStoreError> {
    let (id, token, hash) = (
        registration_id.to_owned(),
        push_token.to_owned(),
        hash.to_owned(),
    );
    store
        .write(move |tx| {
            tx.update::<IosControlRegistration>(
                &id,
                |mut current| {
                    if current.push_token == token {
                        current.last_delivered_hash = Some(hash);
                    }
                    current
                },
                ModifyOpts::default(),
            )
            .map(|_| ())
        })
        .await
        .map_err(fail("mark iOS control delivered"))
}

/// Deletes the registration only if the token still matches (a re-registered
/// token is never removed by a stale rejection).
pub async fn delete_registration(
    store: &Store,
    registration_id: &str,
    push_token: &str,
) -> Result<(), RegistrationStoreError> {
    let (id, token) = (registration_id.to_owned(), push_token.to_owned());
    store
        .write(move |tx| {
            let current = tx.get::<IosControlRegistration>(&id)?;
            if current.is_some_and(|c| c.push_token == token) {
                tx.delete::<IosControlRegistration>(&id)?;
            }
            Ok::<(), StoreError>(())
        })
        .await
        .map_err(fail("delete iOS control registration"))
}
