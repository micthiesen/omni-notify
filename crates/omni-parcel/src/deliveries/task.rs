//! `ParcelDeliveries`: the only caller of Parcel's deliveries endpoint.
//!
//! It ticks every ten minutes, but reads only when [`state::reserve`] allows:
//! every 30 minutes while anything is active, every 3 hours otherwise, never
//! during a 429 back-off and never more than [`state::HOURLY_BUDGET`] times in
//! an hour. A manual run goes through the same gate, so it cannot force a read.
//! In `SideEffectMode::Record` (tests, shadow runs, preview) it never reads,
//! so a second instance cannot spend the shared key's budget.

use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use jiff::tz::TimeZone;
use omni_core::clock::SharedClock;
use omni_core::js::to_iso_string;
use omni_core::spawn::must_complete;
use omni_http::SideEffectMode;
use omni_store::Store;
use omni_tasks::{CronSchedule, InvalidScheduleError, RunContext, Task, TaskError, TaskOptions};
use tokio_util::task::TaskTracker;
use tracing::Instrument as _;

use super::api::{DeliveriesClient, DeliveriesError};
use super::state::{self, MAX_BACKOFF_MS, RATE_LIMIT_BACKOFF_MS, ReadDecision, SkipReason};
use crate::carriers::carrier_map::CarrierDirectory;

pub const TASK_NAME: &str = "ParcelDeliveries";
pub const SCHEDULE: &str = "0 */10 * * * *";
const LOG: &str = "ParcelDeliveries";

#[derive(Debug, thiserror::Error)]
pub enum DeliveriesTaskError {
    #[error(transparent)]
    Store(#[from] omni_store::StoreError),
    #[error(transparent)]
    Read(#[from] DeliveriesError),
}

/// What one tick did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TickOutcome {
    /// Side effects are recorded; Parcel was not read.
    Disabled,
    Skipped {
        reason: SkipReason,
        until: i64,
    },
    Read {
        deliveries: usize,
        active: usize,
    },
    RateLimited {
        until: i64,
    },
    /// A transient failure; the next due tick retries.
    Failed {
        message: String,
    },
}

pub struct DeliveriesTask {
    schedule: CronSchedule,
    store: Store,
    client: DeliveriesClient,
    carriers: Arc<CarrierDirectory>,
    clock: SharedClock,
    tracker: TaskTracker,
    mode: SideEffectMode,
    summary: Mutex<Option<String>>,
}

pub struct DeliveriesTaskDeps {
    pub store: Store,
    pub client: DeliveriesClient,
    pub carriers: Arc<CarrierDirectory>,
    pub clock: SharedClock,
    pub tracker: TaskTracker,
    pub mode: SideEffectMode,
    pub tz: TimeZone,
}

impl DeliveriesTask {
    pub fn new(deps: DeliveriesTaskDeps) -> Result<Self, InvalidScheduleError> {
        Ok(Self {
            schedule: CronSchedule::parse(SCHEDULE, &deps.tz)?,
            store: deps.store,
            client: deps.client,
            carriers: deps.carriers,
            clock: deps.clock,
            tracker: deps.tracker,
            mode: deps.mode,
            summary: Mutex::new(None),
        })
    }

    /// One gated tick. Non-transient read failures are errors; a 429 and
    /// transient failures are recorded and reported as outcomes (the run
    /// reports them degraded, so they stay visible in run history).
    pub async fn tick(&self) -> Result<TickOutcome, DeliveriesTaskError> {
        if self.mode == SideEffectMode::Record {
            return Ok(TickOutcome::Disabled);
        }
        let now = self.clock.now_ms();
        let reservation = state::reserve(&self.store, now).await?;
        if let ReadDecision::Skip { reason, until } = reservation.decision {
            return Ok(TickOutcome::Skipped { reason, until });
        }
        let store = self.store.clone();
        let client = self.client.clone();
        let carriers = self.carriers.clone();
        let clock = self.clock.clone();
        must_complete(
            &self.tracker,
            async move { read_and_record(store, client, carriers, clock).await }
                .instrument(tracing::Span::current()),
        )
        .await
    }

    fn set_summary(&self, summary: String) {
        *self.summary.lock().unwrap_or_else(|p| p.into_inner()) = Some(summary);
    }
}

async fn read_and_record(
    store: Store,
    client: DeliveriesClient,
    carriers: Arc<CarrierDirectory>,
    clock: SharedClock,
) -> Result<TickOutcome, DeliveriesTaskError> {
    match client.fetch().await {
        Ok(raw) => {
            let names = carriers.names().await;
            let cached = state::to_cached(raw, |code| names.get(code).cloned());
            let snapshot = state::record_success(&store, clock.now_ms(), cached).await?;
            Ok(TickOutcome::Read {
                deliveries: snapshot.deliveries.len(),
                active: snapshot.active_count(),
            })
        }
        Err(DeliveriesError::RateLimited { retry_after_ms }) => {
            let pause = retry_after_ms
                .unwrap_or(0)
                .clamp(RATE_LIMIT_BACKOFF_MS, MAX_BACKOFF_MS);
            let until = clock.now_ms() + pause;
            let error = DeliveriesError::RateLimited { retry_after_ms };
            state::record_failure(&store, &error.to_string(), error.status(), Some(until)).await?;
            Ok(TickOutcome::RateLimited { until })
        }
        Err(error) => {
            state::record_failure(&store, &error.to_string(), error.status(), None).await?;
            if error.is_transient() {
                Ok(TickOutcome::Failed {
                    message: error.to_string(),
                })
            } else {
                Err(error.into())
            }
        }
    }
}

impl Task for DeliveriesTask {
    fn name(&self) -> &str {
        TASK_NAME
    }

    fn schedule(&self) -> &CronSchedule {
        &self.schedule
    }

    fn options(&self) -> TaskOptions {
        TaskOptions {
            jitter: std::time::Duration::ZERO,
            run_on_startup: true,
        }
    }

    fn run<'a>(&'a self, _cx: &'a RunContext) -> BoxFuture<'a, Result<(), TaskError>> {
        Box::pin(async move {
            let outcome = match self.tick().await {
                Ok(outcome) => outcome,
                Err(error) => {
                    tracing::error!(target: LOG, "Parcel deliveries read failed: {error}");
                    self.set_summary(format!("Read failed: {error}"));
                    return Err(TaskError::from_error(error));
                }
            };
            let summary = match outcome {
                TickOutcome::Disabled => {
                    "Skipped: side effects are recorded, so Parcel is not read".to_owned()
                }
                TickOutcome::Skipped { reason, until } => {
                    let why = match reason {
                        SkipReason::BackingOff => "rate-limit back-off",
                        SkipReason::NotDue => "not due",
                        SkipReason::BudgetExhausted => "hourly read budget used",
                    };
                    tracing::debug!(target: LOG, "Skipped ({why}) until {}", to_iso_string(until));
                    format!("Skipped ({why}); next read after {}", to_iso_string(until))
                }
                TickOutcome::Read { deliveries, active } => {
                    tracing::info!(
                        target: LOG,
                        "Read {deliveries} Parcel deliveries ({active} active)"
                    );
                    format!("Read {deliveries} deliveries ({active} active)")
                }
                TickOutcome::RateLimited { until } => {
                    tracing::warn!(
                        target: LOG,
                        "Parcel rate limit reached; no reads until {}",
                        to_iso_string(until)
                    );
                    let summary = format!("Rate limited; next read after {}", to_iso_string(until));
                    omni_tasks::report_degraded(summary.clone());
                    summary
                }
                TickOutcome::Failed { message } => {
                    tracing::warn!(target: LOG, "Parcel deliveries read failed: {message}");
                    let summary = format!("Read failed: {message}");
                    omni_tasks::report_degraded(summary.clone());
                    summary
                }
            };
            self.set_summary(summary);
            Ok(())
        })
    }

    fn last_run_summary(&self) -> Option<String> {
        self.summary
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
}
