//! The `LiveDirectory` port (used by MCP livestream tools, the dashboard
//! snapshot and livestream intelligence).

use futures::future::BoxFuture;
use omni_api::streamers::LivestreamDetails;
use omni_runtime::ports::{LiveDirectory, PortError};
use omni_store::Store;
use serde::Serialize;
use serde_json::Value;

use crate::display::{
    display_views, livestream_summary, load_statuses, metrics_view, session_view, status_view,
};
use crate::error::LiveError;
use crate::metrics::{get_platform_viewer_metrics, get_viewer_metrics};
use crate::sessions::get_sessions;
use crate::status::get_status;
use crate::streamers::Roster;

/// Reads the shared roster and the docstore; never polls platforms.
#[derive(Clone)]
pub struct LiveDirectoryService {
    store: Store,
    roster: Roster,
}

impl LiveDirectoryService {
    pub fn new(store: Store, roster: Roster) -> Self {
        Self { store, roster }
    }
}

fn port_error(error: LiveError) -> PortError {
    PortError::Failed {
        message: error.to_string(),
        transient: true,
    }
}

fn to_values<T: Serialize>(items: &[T]) -> Result<Vec<Value>, PortError> {
    items
        .iter()
        .map(|item| {
            serde_json::to_value(item).map_err(|e| PortError::Failed {
                message: e.to_string(),
                transient: false,
            })
        })
        .collect()
}

impl LiveDirectory for LiveDirectoryService {
    /// `LivestreamSummary` per streamer, in roster order (channels.json, then
    /// DGG discoveries).
    fn streamers(&self) -> BoxFuture<'_, Result<Vec<Value>, PortError>> {
        Box::pin(async move {
            let streamers = self.roster.snapshot();
            let statuses = load_statuses(&self.store, &streamers, false)
                .await
                .map_err(port_error)?;
            let summaries: Vec<_> = streamers
                .iter()
                .zip(&statuses)
                .map(|(streamer, (status, _))| livestream_summary(streamer, status))
                .collect();
            to_values(&summaries)
        })
    }

    /// `StreamerStatusView` per streamer, in roster order.
    fn statuses(&self) -> BoxFuture<'_, Result<Vec<Value>, PortError>> {
        Box::pin(async move {
            let streamers = self.roster.snapshot();
            let statuses = load_statuses(&self.store, &streamers, false)
                .await
                .map_err(port_error)?;
            let views: Vec<_> = statuses
                .iter()
                .map(|(status, _)| status_view(status))
                .collect();
            to_values(&views)
        })
    }

    /// `StreamerView`s in dashboard order.
    fn display(&self) -> BoxFuture<'_, Result<Vec<Value>, PortError>> {
        Box::pin(async move {
            let views = display_views(&self.store, &self.roster.snapshot())
                .await
                .map_err(port_error)?;
            to_values(&views)
        })
    }

    /// `LivestreamDetails`, or `None` for an unknown streamer.
    fn details<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<Option<Value>, PortError>> {
        Box::pin(async move {
            let Some(streamer) = self.roster.get(id) else {
                return Ok(None);
            };
            let status = get_status(&self.store, id).await.map_err(port_error)?;
            let aggregate = get_viewer_metrics(&self.store, id)
                .await
                .map_err(port_error)?;
            let platforms = get_platform_viewer_metrics(&self.store, id)
                .await
                .map_err(port_error)?;
            let sessions = get_sessions(&self.store, id).await.map_err(port_error)?;
            let details = LivestreamDetails {
                livestream: livestream_summary(&streamer, &status),
                metrics: metrics_view(&aggregate, &platforms),
                sessions: sessions.sessions.iter().map(session_view).collect(),
            };
            serde_json::to_value(details)
                .map(Some)
                .map_err(|e| PortError::Failed {
                    message: e.to_string(),
                    transient: false,
                })
        })
    }
}
