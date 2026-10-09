//! The shared one-minute reset alert task (`src/reset-alerts/task.ts`).
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
        })
    }

    fn set_summary(&self, summary: Option<String>) {
        *self
            .last_summary
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = summary;
    }

    /// One poll: read, log the snapshot, deliver, summarize.
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
        provider_log!(P::PROVIDER, info, snapshot = %fields, "Reset source snapshot");
        let counts = self.ledger.deliver(&snapshot.alerts, snapshot.now).await?;
        let summary = format!(
            "{}: {} sent, {} already handled, {} uncertain; {} current signals",
            self.source_label,
            counts.sent,
            counts.skipped,
            counts.uncertain,
            snapshot.alerts.len()
        );
        provider_log!(P::PROVIDER, info, "{summary}");
        self.set_summary(Some(summary));
        Ok(counts)
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
