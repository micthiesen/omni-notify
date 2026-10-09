//! Viewer-record tracking and notifications (`ViewerMetricsService.ts`).
//!
//! A new peak becomes a record only after the count falls 5 percent below
//! it (hysteresis), or when the stream goes offline (flush). Every window is
//! tracked and persisted; the record scope only narrows which confirmations
//! may notify. Pending peaks are in memory, as in TS.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use jiff::tz::TimeZone;
use omni_store::Store;
use omni_store::entity::{EntityWrite, UpsertOpts};

use super::persistence::{ViewerMetrics, get_viewer_metrics};
use super::windows::{
    MetricWindow, WindowConfig, calculate_window_max, prune_buckets, update_daily_bucket,
};
use crate::error::LiveError;
use crate::format::{format_count, group_digits};
use crate::notification_policy::ViewerRecordScope;
use crate::notify::{LiveMessage, LiveNotifier};
use crate::platform::NotificationUrlFields;

const HYSTERESIS: f64 = 0.95;
const MAX_BUCKET_AGE_DAYS: i64 = 100;
const LOG: &str = "ViewerMetrics";

/// An unconfirmed new high for one window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingPeak {
    pub value: i64,
    pub previous_max: i64,
}

/// A record ready to notify.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConfirmedPeak {
    pub window: MetricWindow,
    pub config: WindowConfig,
    pub peak: i64,
    pub previous: i64,
}

/// One summed-count observation and how to notify about it.
#[derive(Clone, Debug)]
pub struct ViewerObservation {
    pub streamer_id: String,
    pub display_name: String,
    pub viewer_count: i64,
    pub url_fields: NotificationUrlFields,
    pub token: Option<String>,
    pub scope: ViewerRecordScope,
}

type PeakState = BTreeMap<MetricWindow, PendingPeak>;

#[derive(Clone)]
pub struct ViewerMetricsService {
    store: Store,
    tz: TimeZone,
    notifier: Arc<dyn LiveNotifier>,
    states: Arc<Mutex<HashMap<String, PeakState>>>,
}

impl ViewerMetricsService {
    pub fn new(store: Store, tz: TimeZone, notifier: Arc<dyn LiveNotifier>) -> Self {
        Self {
            store,
            tz,
            notifier,
            states: Arc::default(),
        }
    }

    fn with_state<R>(&self, f: impl FnOnce(&mut HashMap<String, PeakState>) -> R) -> R {
        let mut guard = self
            .states
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(&mut guard)
    }

    /// Records a summed count at `now`.
    pub async fn record_viewer_count(
        &self,
        observation: &ViewerObservation,
        now: i64,
    ) -> Result<(), LiveError> {
        let mut metrics = get_viewer_metrics(&self.store, &observation.streamer_id).await?;
        let count = observation.viewer_count;
        // Window maxima come from the buckets as they stood before this
        // observation. Measuring after it would include `count` itself, so a
        // windowed record could never start (TS had this bug). Earlier
        // observations today still count, so a peak confirmed today, or one
        // observed before a restart, is not reported again at a lower value.
        let prior_buckets = metrics.daily_buckets.clone();
        metrics.daily_buckets = update_daily_bucket(&metrics.daily_buckets, count, now);

        let confirmed = self.with_state(|states| {
            let state = states.entry(observation.streamer_id.clone()).or_default();
            let mut confirmed = Vec::new();
            for window in MetricWindow::ALL {
                let config = window.config();
                let window_max =
                    calculate_window_max(&prior_buckets, metrics.all_time_max, config, now, &self.tz);
                match state.get_mut(&window) {
                    Some(pending) => {
                        #[allow(clippy::cast_precision_loss)]
                        let fell = (count as f64) < pending.value as f64 * HYSTERESIS;
                        if count > pending.value {
                            pending.value = count;
                            tracing::debug!(target: LOG, "{}: Updated pending {} to {count}", observation.streamer_id, config.label);
                        } else if fell {
                            let pending = *pending;
                            state.remove(&window);
                            if window == MetricWindow::AllTime && pending.value > metrics.all_time_max {
                                metrics.all_time_max = pending.value;
                                metrics.all_time_max_timestamp = now;
                            }
                            if observation.scope == ViewerRecordScope::All || window == MetricWindow::AllTime {
                                confirmed.push(ConfirmedPeak {
                                    window,
                                    config,
                                    peak: pending.value,
                                    previous: pending.previous_max,
                                });
                            }
                            tracing::debug!(target: LOG, "{}: Confirmed {} peak at {}", observation.streamer_id, config.label, pending.value);
                        }
                    }
                    None if count > window_max => {
                        state.insert(window, PendingPeak { value: count, previous_max: window_max });
                        tracing::debug!(target: LOG, "{}: Started tracking {} peak at {count} (prev: {window_max})", observation.streamer_id, config.label);
                    }
                    None => {}
                }
            }
            confirmed
        });

        metrics.daily_buckets =
            prune_buckets(&metrics.daily_buckets, MAX_BUCKET_AGE_DAYS, now, &self.tz);
        self.save(metrics).await?;
        self.notify(&confirmed, observation).await
    }

    /// Confirms every pending peak (the stream went offline).
    pub async fn flush_pending_peaks(
        &self,
        observation: &ViewerObservation,
        now: i64,
    ) -> Result<(), LiveError> {
        let pending = self.with_state(|states| {
            states
                .get_mut(&observation.streamer_id)
                .map(std::mem::take)
                .unwrap_or_default()
        });
        if pending.is_empty() {
            return Ok(());
        }
        let mut metrics = get_viewer_metrics(&self.store, &observation.streamer_id).await?;
        let mut confirmed = Vec::new();
        for (window, peak) in pending {
            let config = window.config();
            if window == MetricWindow::AllTime && peak.value > metrics.all_time_max {
                metrics.all_time_max = peak.value;
                metrics.all_time_max_timestamp = now;
            }
            if observation.scope == ViewerRecordScope::All || window == MetricWindow::AllTime {
                confirmed.push(ConfirmedPeak {
                    window,
                    config,
                    peak: peak.value,
                    previous: peak.previous_max,
                });
            }
            tracing::debug!(target: LOG, "{}: Flushed pending {} peak at {}", observation.streamer_id, config.label, peak.value);
        }
        self.save(metrics).await?;
        self.notify(&confirmed, observation).await
    }

    /// Drops unconfirmed observations of a retired transient source.
    pub fn discard_pending_peaks(&self, streamer_id: &str) {
        self.with_state(|states| states.remove(streamer_id));
    }

    async fn save(&self, metrics: ViewerMetrics) -> Result<(), LiveError> {
        self.store
            .write(move |tx| tx.upsert(&metrics, UpsertOpts::default()))
            .await
            .map_err(LiveError::persistence("upsert viewer metrics"))
    }

    async fn notify(
        &self,
        confirmed: &[ConfirmedPeak],
        observation: &ViewerObservation,
    ) -> Result<(), LiveError> {
        let Some(highest) = confirmed.iter().max_by_key(|peak| peak.config.priority) else {
            return Ok(());
        };
        let previous = if highest.previous > 0 {
            format!(" (previous: {})", group_digits(highest.previous))
        } else {
            String::new()
        };
        tracing::info!(target: LOG, "{}: {} at {} viewers", observation.display_name, highest.config.label, highest.peak);
        let message = LiveMessage {
            title: format!(
                "New {} for {}!",
                highest.config.label, observation.display_name
            ),
            message: format!("Peaked at {}{previous}.", format_count(highest.peak)),
            url: Some(observation.url_fields.clone()),
        };
        self.notifier
            .send(observation.token.as_deref(), message)
            .await
            .map_err(LiveError::from)
    }
}
