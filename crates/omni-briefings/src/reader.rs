//! The `BriefingsReader` port (WP12 `briefings_list`) and the `/api/briefings` view.

use futures::future::BoxFuture;
use omni_api::briefings::{BriefingHistory, BriefingNotification, BriefingSummary};
use omni_runtime::ports::{BriefingsReader, PortError};
use omni_store::{Store, StoreError};
use serde_json::Value;

use crate::persistence::{BriefingHistoryData, get_all_histories};

fn newest_first(history: &BriefingHistoryData) -> Vec<BriefingNotification> {
    let mut notifications: Vec<BriefingNotification> = history
        .notifications
        .iter()
        .map(|notification| notification.view())
        .collect();
    notifications.sort_by_key(|row| std::cmp::Reverse(row.timestamp));
    notifications
}

fn newest_timestamp(notifications: &[BriefingNotification]) -> i64 {
    notifications.first().map_or(0, |n| n.timestamp)
}

/// `GET /api/briefings`: each briefing's notifications newest first, briefings
/// ordered by their newest notification.
pub async fn briefing_summaries(store: &Store) -> Result<Vec<BriefingSummary>, StoreError> {
    let mut summaries: Vec<BriefingSummary> = get_all_histories(store)
        .await?
        .iter()
        .map(|history| BriefingSummary {
            name: history.briefing_name.clone(),
            notifications: newest_first(history),
        })
        .collect();
    summaries.sort_by(|a, b| {
        newest_timestamp(&b.notifications).cmp(&newest_timestamp(&a.notifications))
    });
    Ok(summaries)
}

/// Implements [`BriefingsReader`] over the store.
#[derive(Clone)]
pub struct StoreBriefingsReader {
    store: Store,
}

impl StoreBriefingsReader {
    pub fn new(store: Store) -> Self {
        Self { store }
    }
}

impl BriefingsReader for StoreBriefingsReader {
    fn histories(&self) -> BoxFuture<'_, Result<Vec<Value>, PortError>> {
        Box::pin(async move {
            let failed = |message: String| PortError::Failed {
                message,
                transient: false,
            };
            briefing_summaries(&self.store)
                .await
                .map_err(|e| failed(e.to_string()))?
                .into_iter()
                .map(|summary| {
                    serde_json::to_value(BriefingHistory {
                        briefing_name: summary.name,
                        notifications: summary.notifications,
                    })
                    .map_err(|e| failed(e.to_string()))
                })
                .collect()
        })
    }
}
