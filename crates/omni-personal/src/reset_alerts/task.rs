//! The shared one-minute reset alert task.
//! Providers own evidence interpretation; scheduling and delivery are identical.

use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use jiff::tz::TimeZone;
use omni_tasks::{CronSchedule, RunContext, Task, TaskError, TaskOptions};
use serde_json::{Map, Value};

use super::delivery::{
    DeliveryCounts, DeliveryError, Provider, ResetAlert, ResetDeliveryLedger, ResetProvider,
};
use super::source::ResetSourceError;

/// Every minute, at second zero.
pub const RESET_SCHEDULE: &str = "0 * * * * *";

/// Snapshot fields that change on every poll without the source changing.
const VOLATILE_SNAPSHOT_FIELDS: &[&str] = &["feedGeneratedAt"];

/// One provider poll: current signals plus a bounded source snapshot for the log.
#[derive(Clone, Debug, PartialEq)]
pub struct ResetSnapshot {
    pub alerts: Vec<ResetAlert>,
    pub now: i64,
    pub metadata: Map<String, Value>,
}

/// Reads and interprets one provider's sources.
pub trait SnapshotSource: Send + Sync {
    fn read(&self) -> BoxFuture<'_, Result<ResetSnapshot, ResetSourceError>>;
}

/// Why a run failed.
#[derive(Debug, thiserror::Error)]
pub enum ResetRunError {
    #[error(transparent)]
    Source(#[from] ResetSourceError),
    #[error(transparent)]
    Delivery(#[from] DeliveryError),
}

/// A provider's scheduled task.
pub struct ResetAlertTask<P: ResetProvider> {
    name: &'static str,
    display_name: &'static str,
    source_label: &'static str,
    schedule: CronSchedule,
    source: Arc<dyn SnapshotSource>,
    ledger: ResetDeliveryLedger<P>,
    last_summary: Mutex<Option<String>>,
    logged: Mutex<LoggedState>,
}

/// What the last INFO lines reported, so an unchanged poll logs at debug.
#[derive(Default)]
struct LoggedState {
    snapshot: Option<Map<String, Value>>,
    summary: Option<String>,
}

/// The snapshot fields that identify a source change.
fn stable_fields(metadata: &Map<String, Value>) -> Map<String, Value> {
    metadata
        .iter()
        .filter(|(key, _)| !VOLATILE_SNAPSHOT_FIELDS.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

macro_rules! provider_log {
    ($provider:expr, $level:ident, $($arg:tt)+) => {
        match $provider {
            Provider::Codex => tracing::$level!(target: "CodexResets", $($arg)+),
            Provider::Claude => tracing::$level!(target: "ClaudeResets", $($arg)+),
        }
    };
}

impl<P: ResetProvider> ResetAlertTask<P> {
    pub fn new(
        name: &'static str,
        display_name: &'static str,
        source_label: &'static str,
        tz: &TimeZone,
        source: Arc<dyn SnapshotSource>,
        ledger: ResetDeliveryLedger<P>,
    ) -> Result<Self, omni_tasks::InvalidScheduleError> {
        Ok(Self {
            name,
            display_name,
            source_label,
            schedule: CronSchedule::parse(RESET_SCHEDULE, tz)?,
            source,
            ledger,
            last_summary: Mutex::new(None),
            logged: Mutex::new(LoggedState::default()),
        })
    }

    fn set_summary(&self, summary: Option<String>) {
        *self
            .last_summary
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = summary;
    }

    /// One poll: read, log the snapshot, deliver, summarize.
    /// The snapshot and summary log at INFO only when they change (or a
    /// delivery happened); an unchanged poll logs at debug.
    pub async fn run_once(&self) -> Result<DeliveryCounts, ResetRunError> {
        self.set_summary(None);
        let snapshot = self.source.read().await?;
        let mut fields = Map::new();
        fields.insert(
            "fetchedAt".to_owned(),
            Value::String(omni_core::js::to_iso_string(snapshot.now)),
        );
        fields.extend(snapshot.metadata.clone());
        let fields = Value::Object(fields).to_string();
        let stable = stable_fields(&snapshot.metadata);
        let snapshot_changed = {
            let mut logged = self.logged();
            let changed = logged.snapshot.as_ref() != Some(&stable);
            logged.snapshot = Some(stable);
            changed
        };
        if snapshot_changed {
            provider_log!(P::PROVIDER, info, snapshot = %fields, "Reset source snapshot");
        } else {
            provider_log!(P::PROVIDER, debug, snapshot = %fields, "Reset source snapshot");
        }
        let counts = self.ledger.deliver(&snapshot.alerts, snapshot.now).await?;
        let summary = format!(
            "{}: {} sent, {} already handled, {} uncertain; {} current signals",
            self.source_label,
            counts.sent,
            counts.skipped,
            counts.uncertain,
            snapshot.alerts.len()
        );
        let summary_notable = {
            let mut logged = self.logged();
            let changed = logged.summary.as_deref() != Some(summary.as_str());
            logged.summary = Some(summary.clone());
            changed || counts.sent > 0 || counts.uncertain > 0
        };
        if summary_notable {
            provider_log!(P::PROVIDER, info, "{summary}");
        } else {
            provider_log!(P::PROVIDER, debug, "{summary}");
        }
        self.set_summary(Some(summary));
        Ok(counts)
    }

    fn logged(&self) -> std::sync::MutexGuard<'_, LoggedState> {
        self.logged
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl<P: ResetProvider> Task for ResetAlertTask<P> {
    fn name(&self) -> &str {
        self.name
    }

    fn display_name(&self) -> Option<&str> {
        Some(self.display_name)
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
            self.run_once()
                .await
                .map(|_| ())
                .map_err(TaskError::from_error)
        })
    }

    fn last_run_summary(&self) -> Option<String> {
        self.last_summary
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}
