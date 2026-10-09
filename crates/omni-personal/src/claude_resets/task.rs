//! `ClaudeResets` (`src/claude-resets/task.ts`).

use std::sync::Arc;

use futures::future::BoxFuture;
use jiff::tz::TimeZone;
use omni_core::clock::SharedClock;
use omni_http::public::PublicHttpClient;
use serde_json::{Map, Value};

use super::policy::select_claude_reset_alerts;
use super::source::{ClaudeResetSource, read_claude_reset_source};
use crate::js::parse_date;
use crate::reset_alerts::delivery::Claude;
use crate::reset_alerts::{
    ResetAlertTask, ResetDeliveryLedger, ResetSnapshot, ResetSourceError, SnapshotSource,
};

pub const TASK_NAME: &str = "ClaudeResets";

/// Interprets one fetched catalog at `now`.
pub fn interpret(source: &ClaudeResetSource, now: i64, tz: &TimeZone) -> ResetSnapshot {
    let newest = source
        .events
        .iter()
        .enumerate()
        .max_by_key(|(i, e)| {
            // First of the latest dates (a stable descending sort's head).
            (
                parse_date(&e.date, tz).unwrap_or(i64::MIN),
                std::cmp::Reverse(*i),
            )
        })
        .map(|(_, e)| e);
    let mut metadata = Map::new();
    metadata.insert(
        "catalogUpdatedAt".into(),
        Value::String(source.updated.clone()),
    );
    metadata.insert("feedItems".into(), Value::from(source.events.len()));
    if let Some(newest) = newest {
        metadata.insert("newestEventId".into(), Value::String(newest.id.clone()));
        metadata.insert("eventDate".into(), Value::String(newest.date.clone()));
    }
    ResetSnapshot {
        alerts: select_claude_reset_alerts(source, now, tz),
        now,
        metadata,
    }
}

/// Reset Radar reader.
pub struct ClaudeSource {
    pub http: PublicHttpClient,
    pub clock: SharedClock,
    pub tz: TimeZone,
}

impl SnapshotSource for ClaudeSource {
    fn read(&self) -> BoxFuture<'_, Result<ResetSnapshot, ResetSourceError>> {
        Box::pin(async move {
            let source = read_claude_reset_source(&self.http, &self.tz).await?;
            Ok(interpret(&source, self.clock.now_ms(), &self.tz))
        })
    }
}

/// The `ClaudeResets` task.
pub fn claude_reset_task(
    source: Arc<dyn SnapshotSource>,
    ledger: ResetDeliveryLedger<Claude>,
    tz: &TimeZone,
) -> Result<ResetAlertTask<Claude>, omni_tasks::InvalidScheduleError> {
    ResetAlertTask::new(
        TASK_NAME,
        "Claude Code Reset Alerts",
        "Reset Radar",
        tz,
        source,
        ledger,
    )
}
