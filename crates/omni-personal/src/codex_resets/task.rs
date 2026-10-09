//! `CodexResets` (`src/codex-resets/task.ts`).

use std::sync::Arc;

use futures::future::BoxFuture;
use jiff::tz::TimeZone;
use omni_core::clock::SharedClock;
use omni_http::public::PublicHttpClient;
use serde_json::{Map, Value};

use super::history::add_completed_history_alerts;
use super::policy::{is_fresh_feed, select_reset_alerts};
use super::source::{AlertFeed, ResetHistory, read_reset_sources};
use crate::js::parse_date;
use crate::reset_alerts::delivery::Codex;
use crate::reset_alerts::{
    ResetAlertTask, ResetDeliveryLedger, ResetSnapshot, ResetSourceError, SnapshotSource,
};

pub const TASK_NAME: &str = "CodexResets";

/// Interprets one fetched feed and history at `now`.
pub fn interpret(
    feed: &AlertFeed,
    history: &ResetHistory,
    now: i64,
    tz: &TimeZone,
) -> Result<ResetSnapshot, ResetSourceError> {
    if !is_fresh_feed(feed, now, tz) {
        return Err(ResetSourceError::new(
            "check alert feed freshness",
            format!("Feed generated at {}", feed.generated_at),
        ));
    }
    let combined = add_completed_history_alerts(feed, history, now, tz);
    let alerts = select_reset_alerts(&combined, history, now, tz);
    let mut newest_items: Vec<_> = feed.items.iter().collect();
    // Descending by publication time (schema-validated, so every time parses).
    newest_items.sort_by_key(|item| {
        std::cmp::Reverse(parse_date(&item.published_at, tz).unwrap_or(i64::MIN))
    });
    let newest = newest_items.first();
    let mut metadata = Map::new();
    metadata.insert(
        "feedGeneratedAt".into(),
        Value::String(feed.generated_at.clone()),
    );
    metadata.insert("feedItems".into(), Value::from(feed.items.len()));
    metadata.insert("historyItems".into(), Value::from(history.items.len()));
    metadata.insert(
        "completedHistoryFallbacks".into(),
        Value::from(combined.items.len() - feed.items.len()),
    );
    if let Some(newest) = newest {
        metadata.insert("newestAlertId".into(), Value::String(newest.id.clone()));
        if let Some(source) = &newest.source_published_at {
            metadata.insert("sourcePublishedAt".into(), Value::String(source.clone()));
        }
        metadata.insert(
            "alertPublishedAt".into(),
            Value::String(newest.published_at.clone()),
        );
    }
    Ok(ResetSnapshot {
        alerts,
        now,
        metadata,
    })
}

/// Reset Beacon reader.
pub struct CodexSource {
    pub http: PublicHttpClient,
    pub clock: SharedClock,
    pub tz: TimeZone,
}

impl SnapshotSource for CodexSource {
    fn read(&self) -> BoxFuture<'_, Result<ResetSnapshot, ResetSourceError>> {
        Box::pin(async move {
            let (feed, history) = read_reset_sources(&self.http, &self.tz).await?;
            interpret(&feed, &history, self.clock.now_ms(), &self.tz)
        })
    }
}

/// The `CodexResets` task.
pub fn codex_reset_task(
    source: Arc<dyn SnapshotSource>,
    ledger: ResetDeliveryLedger<Codex>,
    tz: &TimeZone,
) -> Result<ResetAlertTask<Codex>, omni_tasks::InvalidScheduleError> {
    ResetAlertTask::new(
        TASK_NAME,
        "Codex Reset Alerts",
        "Reset Beacon",
        tz,
        source,
        ledger,
    )
}
