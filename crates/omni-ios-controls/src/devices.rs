//! App devices (entity `ios-device`, key `deviceId`) and widget push tokens
//! (entity `ios-widget-push`, key `deviceId`).
//!
//! Token removals and delivery marks only apply while the stored token is
//! still the one that was pushed to, so a stale APNs answer never touches a
//! token the app registered meanwhile.

use omni_api::ios::{ApnsEnvironment, IosDeviceKind};
use omni_store::cbor::Extra;
use omni_store::entity::{Entity, EntityOps, EntityWrite, UpsertOpts};
use omni_store::{DocOps, Store, StoreError};
use serde::{Deserialize, Serialize};

use crate::persistence::RegistrationStoreError;

/// At most this many devices (and widget push tokens) are stored.
pub const MAX_DEVICES: usize = 32;

/// One installed copy of the app.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IosDevice {
    pub device_id: String,
    pub name: String,
    pub kind: IosDeviceKind,
    /// `None` keeps the device registered but un-pushable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apns_token: Option<String>,
    pub environment: ApnsEnvironment,
    pub alerts_enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(flatten)]
    pub extra: Extra,
}

impl IosDevice {
    /// Alerts enabled and an APNs token present.
    pub fn receives_alerts(&self) -> bool {
        self.alerts_enabled && self.apns_token.is_some()
    }
}

impl Entity for IosDevice {
    const NAME: &'static str = "ios-device";
    type Key = String;

    fn key(&self) -> String {
        self.device_id.clone()
    }
}

/// A device as the app registers it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInput {
    pub device_id: String,
    pub name: String,
    pub kind: IosDeviceKind,
    pub apns_token: Option<String>,
    pub environment: ApnsEnvironment,
    pub alerts_enabled: bool,
    pub app_version: Option<String>,
}

/// The widget extension's WidgetKit push token for one device.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IosWidgetPush {
    pub device_id: String,
    pub push_token: String,
    pub environment: ApnsEnvironment,
    /// Hash of the On Air content last delivered to this token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_delivered_hash: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity for IosWidgetPush {
    const NAME: &'static str = "ios-widget-push";
    type Key = String;

    fn key(&self) -> String {
        self.device_id.clone()
    }
}

/// A store write that did not happen because of a bound.
#[derive(Debug, thiserror::Error)]
pub enum DeviceWriteError {
    #[error("too many registered devices (at most {MAX_DEVICES})")]
    TooMany,
    #[error(transparent)]
    Store(#[from] RegistrationStoreError),
}

fn fail(operation: &'static str) -> impl Fn(StoreError) -> RegistrationStoreError {
    move |source| RegistrationStoreError { operation, source }
}

pub async fn list_devices(store: &Store) -> Result<Vec<IosDevice>, RegistrationStoreError> {
    store
        .read(|docs| docs.get_all::<IosDevice>())
        .await
        .map_err(fail("read iOS devices"))
}

pub async fn list_widget_pushes(
    store: &Store,
) -> Result<Vec<IosWidgetPush>, RegistrationStoreError> {
    store
        .read(|docs| docs.get_all::<IosWidgetPush>())
        .await
        .map_err(fail("read iOS widget push tokens"))
}

/// Inserts or replaces a device, keeping its creation time and unknown fields.
pub async fn upsert_device(
    store: &Store,
    input: DeviceInput,
) -> Result<IosDevice, DeviceWriteError> {
    let outcome = store
        .write(move |tx| {
            let now = tx.now_ms();
            let previous = tx.get::<IosDevice>(&input.device_id)?;
            if previous.is_none() && tx.get_all::<IosDevice>()?.len() >= MAX_DEVICES {
                return Ok(None);
            }
            let row = IosDevice {
                device_id: input.device_id,
                name: input.name,
                kind: input.kind,
                apns_token: input.apns_token,
                environment: input.environment,
                alerts_enabled: input.alerts_enabled,
                app_version: input.app_version,
                created_at: previous.as_ref().map_or(now, |p| p.created_at),
                updated_at: now,
                extra: previous.map(|p| p.extra).unwrap_or_default(),
            };
            tx.upsert(&row, UpsertOpts::default())?;
            Ok::<_, StoreError>(Some(row))
        })
        .await
        .map_err(fail("save iOS device"))?;
    outcome.ok_or(DeviceWriteError::TooMany)
}

/// Clears a device's APNs token after APNs reported it invalid.
pub async fn clear_device_token(
    store: &Store,
    device_id: &str,
    apns_token: &str,
) -> Result<(), RegistrationStoreError> {
    let (id, token) = (device_id.to_owned(), apns_token.to_owned());
    store
        .write(move |tx| {
            if let Some(mut device) = tx.get::<IosDevice>(&id)?
                && device.apns_token.as_deref() == Some(token.as_str())
            {
                device.apns_token = None;
                device.updated_at = tx.now_ms();
                tx.upsert(&device, UpsertOpts::default())?;
            }
            Ok::<(), StoreError>(())
        })
        .await
        .map_err(fail("clear iOS device token"))
}

/// Replaces a device's widget push token. The delivered hash survives only
/// when token and environment are unchanged.
pub async fn upsert_widget_push(
    store: &Store,
    device_id: &str,
    push_token: &str,
    environment: ApnsEnvironment,
) -> Result<IosWidgetPush, DeviceWriteError> {
    let (id, token) = (device_id.to_owned(), push_token.to_owned());
    let outcome = store
        .write(move |tx| {
            let now = tx.now_ms();
            let previous = tx.get::<IosWidgetPush>(&id)?;
            if previous.is_none() && tx.get_all::<IosWidgetPush>()?.len() >= MAX_DEVICES {
                return Ok(None);
            }
            let unchanged = previous
                .as_ref()
                .is_some_and(|p| p.push_token == token && p.environment == environment);
            let row = IosWidgetPush {
                device_id: id,
                push_token: token,
                environment,
                last_delivered_hash: if unchanged {
                    previous
                        .as_ref()
                        .and_then(|p| p.last_delivered_hash.clone())
                } else {
                    None
                },
                created_at: previous.as_ref().map_or(now, |p| p.created_at),
                updated_at: now,
                extra: previous.map(|p| p.extra).unwrap_or_default(),
            };
            tx.upsert(&row, UpsertOpts::default())?;
            Ok::<_, StoreError>(Some(row))
        })
        .await
        .map_err(fail("save iOS widget push token"))?;
    outcome.ok_or(DeviceWriteError::TooMany)
}

/// Records the delivered On Air hash while the token is still the one pushed to.
pub async fn mark_widget_delivered(
    store: &Store,
    device_id: &str,
    push_token: &str,
    hash: &str,
) -> Result<(), RegistrationStoreError> {
    let (id, token, hash) = (device_id.to_owned(), push_token.to_owned(), hash.to_owned());
    store
        .write(move |tx| {
            if let Some(mut row) = tx.get::<IosWidgetPush>(&id)?
                && row.push_token == token
            {
                row.last_delivered_hash = Some(hash);
                tx.upsert(&row, UpsertOpts::default())?;
            }
            Ok::<(), StoreError>(())
        })
        .await
        .map_err(fail("mark iOS widget push delivered"))
}

/// Deletes a widget push token APNs reported invalid (only if unchanged).
pub async fn delete_widget_push(
    store: &Store,
    device_id: &str,
    push_token: &str,
) -> Result<(), RegistrationStoreError> {
    let (id, token) = (device_id.to_owned(), push_token.to_owned());
    store
        .write(move |tx| {
            if tx
                .get::<IosWidgetPush>(&id)?
                .is_some_and(|row| row.push_token == token)
            {
                tx.delete::<IosWidgetPush>(&id)?;
            }
            Ok::<(), StoreError>(())
        })
        .await
        .map_err(fail("delete iOS widget push token"))
}
