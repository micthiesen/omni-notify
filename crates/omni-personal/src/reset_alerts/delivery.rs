//! Durable once-per-key Pushover delivery.
//!
//! Each alert reserves its key and source-post aliases in one transaction
//! before the provider call. `sending` rows are never resent automatically
//! (the outcome is uncertain); a definite 4xx rejection releases them so the
//! next poll retries. Codex keeps its original `codex-reset-delivery`
//! namespace and keys; Claude uses `claude-reset-delivery`.

use std::marker::PhantomData;
use std::sync::Arc;

use futures::future::BoxFuture;
use omni_alerts::{PushOutcome, Pushover, PushoverChannel, PushoverMessage};
use omni_store::cbor::{self, Extra};
use omni_store::entity::{self, Entity};
use omni_store::{DocMeta, DocOps, DocWrite, Store, StoreError, Tx};
use serde::{Deserialize, Serialize};

/// Reservations expire after 90 days.
pub const DELIVERY_TTL_MS: i64 = 90 * 24 * 60 * 60 * 1000;

/// One push-worthy signal.
#[derive(Clone, Debug, PartialEq)]
pub struct ResetAlert {
    /// Stable identity: event, stage and type (never a re-serialized timestamp).
    pub key: String,
    /// Source-post identities reserved atomically with `key`.
    pub aliases: Vec<String>,
    pub title: String,
    pub message: String,
    /// The original source; the push's "View source" button.
    pub url: String,
    pub occurred_at: i64,
}

impl ResetAlert {
    fn keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = Vec::with_capacity(1 + self.aliases.len());
        for key in std::iter::once(&self.key).chain(&self.aliases) {
            if !keys.contains(key) {
                keys.push(key.clone());
            }
        }
        keys
    }
}

/// Which provider namespace a delivery ledger uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    Codex,
    Claude,
}

/// Compile-time provider marker for the ledger entity.
pub trait ResetProvider: Send + Sync + 'static {
    const PROVIDER: Provider;
    const ENTITY: &'static str;
}

/// `codex-reset-delivery` (unchanged from before the shared-code move).
pub struct Codex;
impl ResetProvider for Codex {
    const PROVIDER: Provider = Provider::Codex;
    const ENTITY: &'static str = "codex-reset-delivery";
}

/// `claude-reset-delivery`.
pub struct Claude;
impl ResetProvider for Claude {
    const PROVIDER: Provider = Provider::Claude;
    const ENTITY: &'static str = "claude-reset-delivery";
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeliveryStatus {
    Sending,
    Sent,
}

/// A reservation row: `{key, status, occurredAt, updatedAt}`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", bound = "")]
pub struct ResetDelivery<P: ResetProvider> {
    pub key: String,
    pub status: DeliveryStatus,
    pub occurred_at: f64,
    pub updated_at: f64,
    #[serde(flatten)]
    pub extra: Extra,
    #[serde(skip)]
    _provider: PhantomData<fn() -> P>,
}

impl<P: ResetProvider> Clone for ResetDelivery<P> {
    fn clone(&self) -> Self {
        Self {
            key: self.key.clone(),
            status: self.status,
            occurred_at: self.occurred_at,
            updated_at: self.updated_at,
            extra: self.extra.clone(),
            _provider: PhantomData,
        }
    }
}

impl<P: ResetProvider> ResetDelivery<P> {
    pub fn new(key: String, status: DeliveryStatus, occurred_at: f64, updated_at: f64) -> Self {
        Self {
            key,
            status,
            occurred_at,
            updated_at,
            extra: Extra::default(),
            _provider: PhantomData,
        }
    }
}

impl<P: ResetProvider> Entity for ResetDelivery<P> {
    const NAME: &'static str = P::ENTITY;
    const DEFAULT_TTL_MS: Option<i64> = Some(DELIVERY_TTL_MS);
    type Key = String;
    fn key(&self) -> String {
        self.key.clone()
    }
}

pub type CodexResetDelivery = ResetDelivery<Codex>;
pub type ClaudeResetDelivery = ResetDelivery<Claude>;

/// A failed provider call, classified for the ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotifyError {
    /// The provider definitely refused (4xx); nothing was delivered.
    Rejected { status: u16, body: String },
    /// The outcome is unknown (timeout, socket error, 5xx).
    Uncertain(String),
}

impl std::fmt::Display for NotifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NotifyError::Rejected { status, body } => {
                write!(f, "Pushover API returned status code {status}: {body}")
            }
            NotifyError::Uncertain(message) => f.write_str(message),
        }
    }
}

/// The push channel reset alerts use (a seam for tests).
pub trait ResetNotifier: Send + Sync {
    /// Both `PUSHOVER_USER` and `PUSHOVER_TOKEN` are configured.
    fn enabled(&self) -> bool;
    fn notify<'a>(&'a self, alert: &'a ResetAlert) -> BoxFuture<'a, Result<(), NotifyError>>;
}

/// Pushover with the general `PUSHOVER_TOKEN`; honors `SideEffectMode::Record`
/// through [`Pushover`].
pub struct PushoverNotifier {
    pushover: Pushover,
}

impl PushoverNotifier {
    pub fn new(pushover: Pushover) -> Self {
        Self { pushover }
    }
}

impl ResetNotifier for PushoverNotifier {
    fn enabled(&self) -> bool {
        self.pushover.is_configured() && self.pushover.has_token(PushoverChannel::General)
    }

    fn notify<'a>(&'a self, alert: &'a ResetAlert) -> BoxFuture<'a, Result<(), NotifyError>> {
        Box::pin(async move {
            let message = PushoverMessage {
                message: alert.message.clone(),
                title: Some(alert.title.clone()),
                url: Some(alert.url.clone()),
                url_title: Some("View source".to_owned()),
                priority: None,
                sound: None,
                timestamp: None,
            };
            send_general(&self.pushover, message).await
        })
    }
}

/// Sends on the General channel, classifying failures for a delivery ledger:
/// a 4xx or an unsent message is a definite rejection, anything else uncertain.
pub async fn send_general(
    pushover: &Pushover,
    message: PushoverMessage,
) -> Result<(), NotifyError> {
    match pushover.send(PushoverChannel::General, message).await {
        Ok(PushOutcome::Sent | PushOutcome::Recorded) => Ok(()),
        Ok(outcome @ (PushOutcome::SkippedNoToken | PushOutcome::Disabled)) => {
            Err(NotifyError::Rejected {
                status: 0,
                body: format!("Pushover did not send the message ({outcome:?})"),
            })
        }
        Err(error) => match error.status {
            Some(status) if error.is_definite_rejection() => Err(NotifyError::Rejected {
                status,
                body: error.body,
            }),
            _ => Err(NotifyError::Uncertain(error.to_string())),
        },
    }
}

/// A reset alert delivery failure, or a ledger storage failure.
#[derive(Debug, thiserror::Error)]
pub enum DeliveryError {
    #[error("Reset alert {key} delivery failed: {cause}")]
    Delivery {
        key: String,
        cause: String,
        uncertain: bool,
    },
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// `{sent, skipped, uncertain}` of one delivery pass.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DeliveryCounts {
    pub sent: u32,
    pub skipped: u32,
    pub uncertain: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reservation {
    Reserved,
    Sending,
    Sent,
}

/// The durable ledger of one provider.
pub struct ResetDeliveryLedger<P: ResetProvider> {
    store: Store,
    notifier: Arc<dyn ResetNotifier>,
    _provider: PhantomData<fn() -> P>,
}

impl<P: ResetProvider> Clone for ResetDeliveryLedger<P> {
    fn clone(&self) -> Self {
        Self {
            store: self.store.clone(),
            notifier: self.notifier.clone(),
            _provider: PhantomData,
        }
    }
}

fn decode_delivery<P: ResetProvider>(
    pk: &str,
    raw: &omni_store::RawRow,
) -> Result<ResetDelivery<P>, StoreError> {
    cbor::from_value(raw.decode()?).map_err(|source| StoreError::Decode {
        pk: pk.to_owned(),
        source,
    })
}

fn write_delivery<P: ResetProvider>(
    tx: &mut Tx<'_>,
    pk: &str,
    record: &ResetDelivery<P>,
    now: i64,
) -> Result<(), StoreError> {
    let value = cbor::to_value(record).map_err(|source| StoreError::Encode {
        pk: pk.to_owned(),
        source,
    })?;
    tx.upsert_doc(
        pk,
        &value,
        DocMeta {
            entity: Some(P::ENTITY.to_owned()),
            version: 0,
            expires_at: Some(now + DELIVERY_TTL_MS),
            updated_at: Some(now),
        },
    )
}

#[allow(clippy::cast_precision_loss)]
fn ms(value: i64) -> f64 {
    value as f64
}

impl<P: ResetProvider> ResetDeliveryLedger<P> {
    pub fn new(store: Store, notifier: Arc<dyn ResetNotifier>) -> Self {
        Self {
            store,
            notifier,
            _provider: PhantomData,
        }
    }

    async fn reserve(&self, alert: &ResetAlert, now: i64) -> Result<Reservation, StoreError> {
        let keys = alert.keys();
        let primary = alert.key.clone();
        let occurred_at = alert.occurred_at;
        self.store
            .write(move |tx| {
                let mut current: Vec<(String, ResetDelivery<P>)> = Vec::new();
                for key in &keys {
                    let pk = entity::pk::<ResetDelivery<P>>(key)?;
                    if let Some(raw) = tx.get_raw_row(&pk)? {
                        let delivery = decode_delivery::<P>(&pk, &raw)?;
                        if &delivery.key != key {
                            return Err(StoreError::Validation {
                                entity: P::ENTITY,
                                reason: "Reset alert key mismatch".to_owned(),
                            });
                        }
                        current.push((key.clone(), delivery));
                    }
                }
                let status = if current
                    .iter()
                    .any(|(_, d)| d.status == DeliveryStatus::Sending)
                {
                    Some(DeliveryStatus::Sending)
                } else if current.is_empty() {
                    None
                } else {
                    Some(DeliveryStatus::Sent)
                };
                let template = current.first().map(|(_, d)| d);
                let record_status = status.unwrap_or(DeliveryStatus::Sending);
                let record_occurred = template.map_or(ms(occurred_at), |d| d.occurred_at);
                let record_updated = template.map_or(ms(now), |d| d.updated_at);
                let primary_exists = current.iter().any(|(key, _)| *key == primary);
                // An in-flight alias may belong to another alert's primary key. Do not
                // create this alert's primary reservation: its owner settles the shared
                // alias when its provider call finishes.
                if !primary_exists && status == Some(DeliveryStatus::Sending) {
                    return Ok(Reservation::Sending);
                }
                for key in &keys {
                    if current.iter().any(|(existing, _)| existing == key) {
                        continue;
                    }
                    let pk = entity::pk::<ResetDelivery<P>>(key)?;
                    let record = ResetDelivery::<P>::new(
                        key.clone(),
                        record_status,
                        record_occurred,
                        record_updated,
                    );
                    write_delivery(tx, &pk, &record, now)?;
                }
                Ok(match status {
                    Some(DeliveryStatus::Sending) => Reservation::Sending,
                    Some(DeliveryStatus::Sent) => Reservation::Sent,
                    None => Reservation::Reserved,
                })
            })
            .await
    }

    async fn mark_sent(&self, alert: &ResetAlert, now: i64) -> Result<(), StoreError> {
        let keys = alert.keys();
        self.store
            .write(move |tx| {
                for key in &keys {
                    let pk = entity::pk::<ResetDelivery<P>>(key)?;
                    let Some(raw) = tx.get_raw_row(&pk)? else {
                        return Err(StoreError::Validation {
                            entity: P::ENTITY,
                            reason: "Reset alert reservation expired before acknowledgement"
                                .to_owned(),
                        });
                    };
                    let mut current = decode_delivery::<P>(&pk, &raw)?;
                    current.status = DeliveryStatus::Sent;
                    current.updated_at = ms(now);
                    write_delivery(tx, &pk, &current, now)?;
                }
                Ok(())
            })
            .await
    }

    async fn release_definite_rejection(&self, alert: &ResetAlert) -> Result<(), StoreError> {
        let keys = alert.keys();
        self.store
            .write(move |tx| {
                for key in &keys {
                    let pk = entity::pk::<ResetDelivery<P>>(key)?;
                    let Some(raw) = tx.get_raw_row(&pk)? else {
                        continue;
                    };
                    if decode_delivery::<P>(&pk, &raw)?.status == DeliveryStatus::Sending {
                        tx.delete_doc(&pk)?;
                    }
                }
                Ok(())
            })
            .await
    }

    /// Delivers each alert once per stable key, preserving ambiguous attempts.
    /// Stops at the first failed provider call.
    pub async fn deliver(
        &self,
        alerts: &[ResetAlert],
        now: i64,
    ) -> Result<DeliveryCounts, DeliveryError> {
        let mut counts = DeliveryCounts::default();
        if !self.notifier.enabled() {
            return Err(DeliveryError::Delivery {
                key: "configuration".to_owned(),
                cause: "Pushover is disabled; reset alerts cannot be delivered".to_owned(),
                uncertain: false,
            });
        }
        for alert in alerts {
            match self.reserve(alert, now).await? {
                Reservation::Sent => {
                    counts.skipped += 1;
                    continue;
                }
                Reservation::Sending => {
                    counts.uncertain += 1;
                    continue;
                }
                Reservation::Reserved => {}
            }
            match self.notifier.notify(alert).await {
                Ok(()) => {
                    self.mark_sent(alert, now).await?;
                    counts.sent += 1;
                }
                Err(error @ NotifyError::Rejected { .. }) => {
                    self.release_definite_rejection(alert).await?;
                    return Err(DeliveryError::Delivery {
                        key: alert.key.clone(),
                        cause: error.to_string(),
                        uncertain: false,
                    });
                }
                Err(error @ NotifyError::Uncertain(_)) => {
                    return Err(DeliveryError::Delivery {
                        key: alert.key.clone(),
                        cause: error.to_string(),
                        uncertain: true,
                    });
                }
            }
        }
        Ok(counts)
    }
}
