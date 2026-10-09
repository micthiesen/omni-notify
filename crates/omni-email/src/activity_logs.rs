//! Captured log lines for emails that reached a pipeline's processing phase
//! (`src/email/activityLogs.ts`). One `email-activity-log` row per activity,
//! overwritten on reprocess and pruned with the activity rows.

use std::future::Future;

use omni_store::cbor::Extra;
use omni_store::entity::{Entity, EntityOps as _, EntityWrite as _, UpsertOpts};
use omni_store::{LogLine, Store, StoreError, logs_gz};
use omni_tasks::RunLogs;
use omni_tasks::log_capture::{CaptureHandle, capture_scope};
use serde::{Deserialize, Serialize};
use tracing::Instrument as _;

const LOG: &str = "EmailActivityLogs";

/// Stored row (entity `email-activity-log`, key `activityId`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailActivityLog {
    pub activity_id: String,
    /// `logs_gz` encoding of the lines.
    pub lines_gz: String,
    /// Oldest lines dropped once the capture cap was hit.
    pub dropped: u64,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity for EmailActivityLog {
    const NAME: &'static str = "email-activity-log";
    type Key = String;
    fn key(&self) -> String {
        self.activity_id.clone()
    }
}

/// Decoded logs for one activity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmailActivityLogData {
    pub activity_id: String,
    pub lines: Vec<LogLine>,
    pub dropped: u64,
}

/// `saveEmailActivityLogs`: an empty capture deletes any stale row.
pub async fn save(store: &Store, data: EmailActivityLogData) -> Result<(), StoreError> {
    if data.lines.is_empty() && data.dropped == 0 {
        let key = data.activity_id;
        store
            .write(move |tx| tx.delete::<EmailActivityLog>(&key).map(|_| ()))
            .await?;
        return Ok(());
    }
    let row = EmailActivityLog {
        activity_id: data.activity_id,
        lines_gz: logs_gz::encode(&data.lines)?,
        dropped: data.dropped,
        extra: Extra::new(),
    };
    store
        .write(move |tx| tx.upsert(&row, UpsertOpts::default()))
        .await
}

/// `getEmailActivityLogs`: an unreadable row is deleted (with a warning) and
/// reported as absent rather than failing the logs endpoint forever.
pub async fn get(
    store: &Store,
    activity_id: &str,
) -> Result<Option<EmailActivityLogData>, StoreError> {
    let key = activity_id.to_owned();
    let read = store
        .read(move |docs| docs.get::<EmailActivityLog>(&key))
        .await
        .and_then(|row| {
            row.map(|row| {
                logs_gz::decode(&row.lines_gz).map(|lines| EmailActivityLogData {
                    activity_id: row.activity_id,
                    lines,
                    dropped: row.dropped,
                })
            })
            .transpose()
        });
    match read {
        Ok(data) => Ok(data),
        Err(error) => {
            let key = activity_id.to_owned();
            store
                .write(move |tx| tx.delete::<EmailActivityLog>(&key))
                .await?;
            tracing::warn!(
                target: LOG,
                "Dropped unreadable log row for \"{activity_id}\": {error}"
            );
            Ok(None)
        }
    }
}

/// Ends the capture even when the processing future is dropped, so the live
/// buffer never leaks.
struct CaptureGuard(Option<CaptureHandle>);

impl CaptureGuard {
    fn finish(mut self) -> (Vec<LogLine>, u64) {
        self.0.take().map(CaptureHandle::finish).unwrap_or_default()
    }
}

impl Drop for CaptureGuard {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            handle.finish();
        }
    }
}

/// `withEmailLogCaptureEffect`: runs `work` with every log line attributed to
/// this email's activity, then persists the capture. A persistence failure is
/// only warned about: the work's own result (success or failure) is returned
/// unchanged.
pub async fn with_capture<F, T>(
    store: &Store,
    logs: &RunLogs,
    activity_id: &str,
    pipeline: &str,
    work: F,
) -> T
where
    F: Future<Output = T>,
{
    let capture_id = format!("{activity_id}:{}", omni_core::ids::uuid_v4());
    let (span, handle) = capture_scope(logs, &capture_id, pipeline);
    let guard = CaptureGuard(Some(handle));
    let result = work.instrument(span).await;
    let (lines, dropped) = guard.finish();
    let saved = save(
        store,
        EmailActivityLogData {
            activity_id: activity_id.to_owned(),
            lines,
            dropped,
        },
    )
    .await;
    if let Err(error) = saved {
        tracing::warn!(
            target: LOG,
            "Could not persist diagnostic log for \"{activity_id}\": {error}"
        );
    }
    result
}
