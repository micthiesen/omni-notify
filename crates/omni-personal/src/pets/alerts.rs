//! Durable pet health alert state and delivery.
//!
//! One `pet-health-alert` row per pet and rule (`*` for the household-wide
//! data gap) is the only throttle for these pushes. Each pass decides inside a
//! write transaction and, when a push is due, records it as `sending` with
//! `lastNotifiedAt` before the provider call. A definite rejection restores the
//! previous row so the next pass retries; an uncertain outcome keeps the
//! reservation and is never resent.

use std::sync::Arc;

use futures::future::BoxFuture;
use omni_alerts::{AlertGate, Pushover, PushoverChannel, PushoverMessage};
use omni_api::pets::{PetHealthAlertInfo, PetHealthKind};
use omni_core::js::to_iso_string;
use omni_store::cbor::Extra;
use omni_store::entity::{Entity, UpsertOpts};
use omni_store::{EntityOps, EntityWrite, Store, StoreError};
use omni_tasks::persistence::{self};
use serde::{Deserialize, Serialize};

use super::health::{Assessment, HOUSEHOLD, Notice, RuleState, decide};
use crate::reset_alerts::NotifyError;
use crate::reset_alerts::delivery::send_general;

/// The push's link.
pub const PETS_URL: &str = "http://omni.boris/pets";

/// The last push's delivery state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PushStatus {
    Sending,
    Sent,
}

/// `pet-health-alert`, keyed `(petId, kind)`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PetHealthAlert {
    /// `*` for household-wide rules.
    pub pet_id: String,
    pub kind: PetHealthKind,
    pub active: bool,
    pub notified: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode_started_at: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_notified_at: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_value: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovered_at: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub push: Option<PushStatus>,
    pub updated_at: f64,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity for PetHealthAlert {
    const NAME: &'static str = "pet-health-alert";
    type Key = (String, String);
    fn key(&self) -> (String, String) {
        (self.pet_id.clone(), self.kind.as_str().to_owned())
    }
}

#[allow(clippy::cast_precision_loss)]
fn ms_f(value: i64) -> f64 {
    value as f64
}

#[allow(clippy::cast_possible_truncation)]
fn ms_i(value: f64) -> i64 {
    value as i64
}

impl PetHealthAlert {
    fn state(&self) -> RuleState {
        RuleState {
            active: self.active,
            notified: self.notified,
            episode_started_at: self.episode_started_at.map(ms_i),
            last_notified_at: self.last_notified_at.map(ms_i),
            last_value: self.last_value,
            last_message: self.last_message.clone(),
            recovered_at: self.recovered_at.map(ms_i),
        }
    }

    fn with_state(
        previous: Option<&PetHealthAlert>,
        pet_id: &str,
        kind: PetHealthKind,
        state: &RuleState,
        now: i64,
    ) -> Self {
        Self {
            pet_id: pet_id.to_owned(),
            kind,
            active: state.active,
            notified: state.notified,
            episode_started_at: state.episode_started_at.map(ms_f),
            last_notified_at: state.last_notified_at.map(ms_f),
            last_value: state.last_value,
            last_message: state.last_message.clone(),
            recovered_at: state.recovered_at.map(ms_f),
            push: previous.and_then(|p| p.push),
            updated_at: ms_f(now),
            extra: previous.map(|p| p.extra.clone()).unwrap_or_default(),
        }
    }

    /// The API view.
    pub fn info(&self) -> PetHealthAlertInfo {
        PetHealthAlertInfo {
            pet_id: (self.pet_id != HOUSEHOLD).then(|| self.pet_id.clone()),
            kind: self.kind,
            active: self.active,
            last_notified_at: self.last_notified_at.map(|at| to_iso_string(ms_i(at))),
            last_message: self.last_message.clone(),
            recovered_at: self.recovered_at.map(|at| to_iso_string(ms_i(at))),
        }
    }
}

/// The push channel pet health alerts use (a seam for tests).
pub trait HealthNotifier: Send + Sync {
    fn notify<'a>(&'a self, notice: &'a Notice) -> BoxFuture<'a, Result<(), NotifyError>>;
}

/// Pushover General; honors `SideEffectMode::Record` through [`Pushover`].
pub struct PushoverHealthNotifier {
    pushover: Pushover,
}

impl PushoverHealthNotifier {
    pub fn new(pushover: Pushover) -> Self {
        Self { pushover }
    }

    /// Both `PUSHOVER_USER` and a General token are configured.
    pub fn enabled(&self) -> bool {
        self.pushover.is_configured() && self.pushover.has_token(PushoverChannel::General)
    }
}

impl HealthNotifier for PushoverHealthNotifier {
    fn notify<'a>(&'a self, notice: &'a Notice) -> BoxFuture<'a, Result<(), NotifyError>> {
        Box::pin(async move {
            let message = PushoverMessage {
                message: notice.message().to_owned(),
                title: Some(notice.title().to_owned()),
                url: Some(PETS_URL.to_owned()),
                url_title: Some("Open pets".to_owned()),
                priority: None,
                sound: None,
                timestamp: None,
            };
            send_general(&self.pushover, message).await
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HealthAlertError {
    #[error("Pet health alert {key} delivery failed: {cause}")]
    Delivery {
        key: String,
        cause: String,
        uncertain: bool,
    },
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// Reads and advances the alert rows; sends through `notifier` when present.
#[derive(Clone)]
pub struct HealthLedger {
    store: Store,
    notifier: Option<Arc<dyn HealthNotifier>>,
}

impl HealthLedger {
    /// `notifier` is `None` when Pushover is not configured: state is then left
    /// untouched so the first configured run still reports open episodes.
    pub fn new(store: Store, notifier: Option<Arc<dyn HealthNotifier>>) -> Self {
        Self { store, notifier }
    }

    /// Every row, newest notification first.
    pub async fn all(&self) -> Result<Vec<PetHealthAlert>, StoreError> {
        let mut rows = self
            .store
            .read(|docs| docs.get_all::<PetHealthAlert>())
            .await?;
        rows.sort_by(|a, b| {
            b.last_notified_at
                .unwrap_or(f64::MIN)
                .total_cmp(&a.last_notified_at.unwrap_or(f64::MIN))
        });
        Ok(rows)
    }

    /// The household data-gap row.
    pub async fn gap(&self) -> Result<Option<PetHealthAlert>, StoreError> {
        let key = (
            HOUSEHOLD.to_owned(),
            PetHealthKind::DataGap.as_str().to_owned(),
        );
        self.store
            .read(move |docs| docs.get::<PetHealthAlert>(&key))
            .await
    }

    /// Advances every assessment's row and sends the pushes that are due.
    /// Stops at the first failed push. Returns the pushes sent.
    pub async fn apply(
        &self,
        assessments: &[Assessment],
        now: i64,
    ) -> Result<u32, HealthAlertError> {
        let Some(notifier) = &self.notifier else {
            return Ok(0);
        };
        let mut sent = 0;
        for assessment in assessments {
            let (previous, notice) = self.advance(assessment, now).await?;
            let Some(notice) = notice else {
                continue;
            };
            let key = format!("{}:{}", assessment.pet_id, assessment.kind.as_str());
            match notifier.notify(&notice).await {
                Ok(()) => {
                    self.set_push(assessment, PushStatus::Sent).await?;
                    sent += 1;
                }
                Err(error @ NotifyError::Rejected { .. }) => {
                    self.restore(assessment, previous).await?;
                    return Err(HealthAlertError::Delivery {
                        key,
                        cause: error.to_string(),
                        uncertain: false,
                    });
                }
                Err(error @ NotifyError::Uncertain(_)) => {
                    return Err(HealthAlertError::Delivery {
                        key,
                        cause: error.to_string(),
                        uncertain: true,
                    });
                }
            }
        }
        Ok(sent)
    }

    /// Decides and writes the next row in one transaction; a due push is
    /// reserved as `sending`. Returns the row before and the push.
    async fn advance(
        &self,
        assessment: &Assessment,
        now: i64,
    ) -> Result<(Option<PetHealthAlert>, Option<Notice>), StoreError> {
        let assessment = assessment.clone();
        self.store
            .write(move |tx| {
                let key = (
                    assessment.pet_id.clone(),
                    assessment.kind.as_str().to_owned(),
                );
                let previous = tx.get::<PetHealthAlert>(&key)?;
                let previous_state = previous.as_ref().map(PetHealthAlert::state);
                let (next, notice) = decide(
                    assessment.kind,
                    previous_state.as_ref(),
                    &assessment.signal,
                    now,
                );
                if let Some(next) = next.filter(|n| Some(n) != previous_state.as_ref()) {
                    let mut row = PetHealthAlert::with_state(
                        previous.as_ref(),
                        &assessment.pet_id,
                        assessment.kind,
                        &next,
                        now,
                    );
                    if notice.is_some() {
                        row.push = Some(PushStatus::Sending);
                    }
                    tx.upsert(&row, UpsertOpts::default())?;
                }
                Ok((previous, notice))
            })
            .await
    }

    async fn set_push(
        &self,
        assessment: &Assessment,
        status: PushStatus,
    ) -> Result<(), StoreError> {
        let key = (
            assessment.pet_id.clone(),
            assessment.kind.as_str().to_owned(),
        );
        self.store
            .write(move |tx| {
                if let Some(mut row) = tx.get::<PetHealthAlert>(&key)? {
                    row.push = Some(status);
                    tx.upsert(&row, UpsertOpts::default())?;
                }
                Ok(())
            })
            .await
    }

    async fn restore(
        &self,
        assessment: &Assessment,
        previous: Option<PetHealthAlert>,
    ) -> Result<(), StoreError> {
        let key = (
            assessment.pet_id.clone(),
            assessment.kind.as_str().to_owned(),
        );
        self.store
            .write(move |tx| {
                match previous {
                    Some(row) => tx.upsert(&row, UpsertOpts::default())?,
                    None => {
                        tx.delete::<PetHealthAlert>(&key)?;
                    }
                }
                Ok(())
            })
            .await
    }
}

/// Run errors that start with this are data gaps the health watch reports.
pub const GAP_ERROR_PREFIX: &str = "No litter-box readings";

/// Keeps a data-gap run failure off the ERROR-log Pushover path while the
/// health watch tracks that gap (its row is active), so the gap is throttled
/// at one layer: one push per gap, at most one per week. Any other PetTracker
/// failure, or a gap the ledger failed to record, still alerts.
pub struct PetGapAlertGate {
    store: Store,
    titles: [String; 3],
}

impl PetGapAlertGate {
    pub fn new(store: Store) -> Self {
        let task = super::task::TASK_NAME;
        Self {
            store,
            titles: [
                format!("Error running task \"{task}\""),
                format!("Manual run of \"{task}\" failed"),
                format!("Catch-up run of \"{task}\" failed"),
            ],
        }
    }
}

impl AlertGate for PetGapAlertGate {
    fn applies(&self, title: &str) -> bool {
        self.titles.iter().any(|t| t == title)
    }

    fn should_notify<'a>(&'a self, _title: &'a str) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            let latest_is_gap = persistence::get_runs(&self.store, Some(super::task::TASK_NAME), 1)
                .await
                .ok()
                .and_then(|runs| runs.into_iter().next())
                .and_then(|run| run.error)
                .is_some_and(|error| error.starts_with(GAP_ERROR_PREFIX));
            if !latest_is_gap {
                return true;
            }
            let tracked = HealthLedger::new(self.store.clone(), None)
                .gap()
                .await
                .ok()
                .flatten()
                .is_some_and(|row| row.active);
            !tracked
        })
    }
}
