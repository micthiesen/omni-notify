//! Durable profile identity links (`identityLinks.ts`, entity
//! `live-profile-identity-link`): a DGG-discovered account resolved to a
//! configured account through verified platform evidence.

use omni_store::Store;
use omni_store::cbor::Extra;
use omni_store::entity::{Entity, EntityOps, EntityWrite, UpsertOpts};
use serde::{Deserialize, Serialize};

use crate::error::LiveError;
use crate::platform::{Platform, PlatformBinding};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileIdentityLink {
    /// Canonical discovered account, e.g. `kick:somebody`.
    pub source_binding: String,
    /// Canonical configured account.
    pub target_binding: String,
    pub discovered_at: i64,
    pub verified_at: i64,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity for ProfileIdentityLink {
    const NAME: &'static str = "live-profile-identity-link";
    type Key = String;

    fn key(&self) -> String {
        self.source_binding.clone()
    }
}

/// Stable account key; handles are case-insensitive on every supported host
/// (YouTube `channel/<id>` ids keep their case).
pub fn canonical_binding_key(platform: Platform, username: &str) -> String {
    let trimmed = crate::streamers::js_trim(username);
    let username = if platform == Platform::YouTube {
        if let Some(handle) = trimmed.strip_prefix('@') {
            format!("@{}", handle.to_lowercase())
        } else if let Some((kind, value)) = trimmed.split_once('/') {
            // JS `split("/", 2)` keeps only the first two segments.
            let value = value.split('/').next().unwrap_or_default();
            format!("{}/{value}", kind.to_lowercase())
        } else {
            trimmed.to_owned()
        }
    } else {
        trimmed.to_lowercase()
    };
    format!("{platform}:{username}")
}

/// `canonicalBindingKey(binding)`.
pub fn binding_key(binding: &PlatformBinding) -> String {
    canonical_binding_key(binding.platform, &binding.username)
}

pub async fn get_link(
    store: &Store,
    source: &PlatformBinding,
) -> Result<Option<ProfileIdentityLink>, LiveError> {
    let key = binding_key(source);
    store
        .read(move |docs| docs.get::<ProfileIdentityLink>(&key))
        .await
        .map_err(LiveError::persistence("read profile identity link"))
}

pub async fn all_links(store: &Store) -> Result<Vec<ProfileIdentityLink>, LiveError> {
    store
        .read(|docs| docs.get_all::<ProfileIdentityLink>())
        .await
        .map_err(LiveError::persistence("list profile identity links"))
}

pub async fn forget_link(store: &Store, source: &PlatformBinding) -> Result<(), LiveError> {
    let key = binding_key(source);
    store
        .write(move |tx| tx.delete::<ProfileIdentityLink>(&key).map(|_| ()))
        .await
        .map_err(LiveError::persistence("delete profile identity link"))
}

/// Upserts the alias, keeping `discoveredAt` while the target is unchanged.
pub async fn remember_link(
    store: &Store,
    source: &PlatformBinding,
    target: &PlatformBinding,
    now: i64,
) -> Result<ProfileIdentityLink, LiveError> {
    let source_binding = binding_key(source);
    let target_binding = binding_key(target);
    store
        .write(move |tx| {
            let existing = tx.get::<ProfileIdentityLink>(&source_binding)?;
            let discovered_at = existing
                .filter(|link| link.target_binding == target_binding)
                .map_or(now, |link| link.discovered_at);
            let row = ProfileIdentityLink {
                source_binding,
                target_binding,
                discovered_at,
                verified_at: now,
                extra: Extra::new(),
            };
            tx.upsert(&row, UpsertOpts::default())?;
            Ok(row)
        })
        .await
        .map_err(LiveError::persistence("upsert profile identity link"))
}
