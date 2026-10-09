//! Castro: the private Tentacles sync protocol, the account client built on
//! it, Inbox cleanup and its failure alert gate. See `docs/castro-sync.md`.

use std::sync::Arc;

use omni_config::Config;
use omni_core::clock::SharedClock;
use omni_http::SideEffectMode;
use omni_http::public::PublicHttpClient;

use crate::account::{AccountProvider, PodcastAccount};

pub mod alert_gate;
pub mod api;
pub mod auth;
pub mod cleanup;
pub mod client;
pub mod fractional;
pub mod protocol;

const LOG: &str = "Castro";

/// Resolves a fresh [`client::CastroClient`] per call over the process-wide
/// paced API (`createCastroClientEffect`).
pub struct CastroAccounts {
    http: PublicHttpClient,
    clock: SharedClock,
    mode: SideEffectMode,
    access_id: String,
    secret: String,
}

impl CastroAccounts {
    /// `None` when Castro is not configured; one half of the credentials warns.
    pub fn from_config(
        config: &Config,
        http: PublicHttpClient,
        clock: SharedClock,
        mode: SideEffectMode,
    ) -> Option<Self> {
        let access_id = config.castro_access_id.clone().filter(|s| !s.is_empty());
        let secret = config.castro_secret_key.clone().filter(|s| !s.is_empty());
        match (access_id, secret) {
            (Some(access_id), Some(secret)) => Some(Self {
                http,
                clock,
                mode,
                access_id,
                secret,
            }),
            (None, None) => None,
            _ => {
                tracing::warn!(target: LOG, "Castro requires both CASTRO_ACCESS_ID and CASTRO_SECRET_KEY");
                None
            }
        }
    }
}

impl AccountProvider for CastroAccounts {
    fn resolve(&self) -> Option<Arc<dyn PodcastAccount>> {
        let api = api::shared_castro_api(
            &self.http,
            &self.clock,
            self.mode,
            &self.access_id,
            &self.secret,
        );
        Some(Arc::new(client::CastroClient::new(api, self.clock.clone())))
    }
}

/// The provider used when Castro is not configured.
pub struct NoAccount;

impl AccountProvider for NoAccount {
    fn resolve(&self) -> Option<Arc<dyn PodcastAccount>> {
        None
    }
}
