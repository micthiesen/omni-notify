//! `omni_runtime::ports::LiveIntelligence` for WP04 (task hooks, routes) and
//! WP12 (`livestream_get`).
//!
//! The port has no per-streamer `observeLive` hook, so `after_tick` pulls the
//! tick's results from the `LiveDirectory` port: every live streamer due this
//! tick (background tier on every third tick, as `isStreamerDue`) is observed,
//! active streamers that are no longer live or listed are taken offline, and
//! then voice targets are scheduled. `on_transition` with `live: false` maps to
//! `observeOffline`.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};

use futures::future::BoxFuture;
use omni_runtime::ports::{LiveIntelligence, LiveTransition, PortError};
use serde::Deserialize;
use serde_json::Value;

use crate::LOG;
use crate::observation::{LiveObservation, LiveStatus, Streamer, StreamerTier};
use crate::routes::{DetailsError, IntelState, parse_feedback, submit_feedback};
use crate::service::LivestreamIntelligenceService;

const BACKGROUND_POLL_FACTOR: u64 = 3;

fn port_error(error: &DetailsError) -> PortError {
    PortError::Failed {
        message: error.to_string(),
        transient: true,
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatusHead {
    streamer_id: String,
    #[serde(default)]
    is_live: bool,
}

/// The port implementation.
pub struct IntelligencePort {
    state: IntelState,
    tick: AtomicU64,
}

impl IntelligencePort {
    pub fn new(state: IntelState) -> Self {
        Self {
            state,
            tick: AtomicU64::new(0),
        }
    }

    async fn observe_tick(&self, service: &LivestreamIntelligenceService) {
        let Some(directory) = self.state.ports.live_directory() else {
            return;
        };
        let (streamers, statuses) = match (directory.streamers().await, directory.statuses().await)
        {
            (Ok(streamers), Ok(statuses)) => (streamers, statuses),
            (Err(error), _) | (_, Err(error)) => {
                tracing::warn!(target: LOG, %error, "Live directory unavailable for intelligence");
                return;
            }
        };
        let tick = self.tick.fetch_add(1, Ordering::SeqCst);
        let mut listed = HashSet::new();
        let mut live = Vec::new();
        for value in streamers {
            match serde_json::from_value::<Streamer>(value) {
                Ok(streamer) => {
                    listed.insert(streamer.id.clone());
                    live.push(streamer);
                }
                Err(error) => tracing::warn!(target: LOG, %error, "Skipping undecodable streamer"),
            }
        }
        let mut live_statuses = std::collections::HashMap::new();
        for value in statuses {
            let Ok(head) = StatusHead::deserialize(&value) else {
                continue;
            };
            if !head.is_live {
                continue;
            }
            match serde_json::from_value::<LiveStatus>(value) {
                Ok(status) => {
                    live_statuses.insert(head.streamer_id, status);
                }
                Err(error) => {
                    tracing::warn!(target: LOG, %error, "Skipping undecodable live status")
                }
            }
        }
        for streamer in live {
            let due = streamer.tier != StreamerTier::Background
                || tick.is_multiple_of(BACKGROUND_POLL_FACTOR);
            match live_statuses.remove(&streamer.id) {
                Some(status) if due => {
                    let name = streamer.display_name.clone();
                    if let Err(error) = service
                        .observe_live(LiveObservation { streamer, status })
                        .await
                    {
                        tracing::error!(target: LOG, "Intelligence observation failed for {name}: {error}");
                    }
                }
                Some(_) => {}
                None if service.is_active(&streamer.id) => {
                    self.offline(service, &streamer.id).await;
                }
                None => {}
            }
        }
        for id in service.active_ids() {
            if !listed.contains(&id) {
                self.offline(service, &id).await;
            }
        }
    }

    async fn offline(&self, service: &LivestreamIntelligenceService, id: &str) {
        if let Err(error) = service.observe_offline(id).await {
            tracing::error!(target: LOG, "Intelligence offline handling failed for {id}: {error}");
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FeedbackPortInput {
    streamer_id: String,
}

impl LiveIntelligence for IntelligencePort {
    fn after_tick(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let Some(service) = self.state.service() else {
                return;
            };
            self.observe_tick(service).await;
            if let Err(error) = service.after_tick().await {
                tracing::error!(target: LOG, "Intelligence voice scheduling failed: {error}");
            }
        })
    }

    fn on_transition<'a>(&'a self, transition: &'a LiveTransition) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            if transition.live {
                return;
            }
            if let Some(service) = self.state.service() {
                self.offline(service, &transition.streamer_id).await;
            }
        })
    }

    fn details<'a>(
        &'a self,
        id: &'a str,
        limit: usize,
    ) -> BoxFuture<'a, Result<Option<Value>, PortError>> {
        Box::pin(async move {
            self.state
                .details_json(id, limit)
                .await
                .map(Some)
                .map_err(|e| port_error(&e))
        })
    }

    fn diagnostics(&self) -> BoxFuture<'_, Result<Value, PortError>> {
        Box::pin(async move { self.state.runtime_json().await.map_err(|e| port_error(&e)) })
    }

    fn record_feedback(&self, input: Value) -> BoxFuture<'_, Result<Value, PortError>> {
        Box::pin(async move {
            let target = FeedbackPortInput::deserialize(&input).map_err(|e| PortError::Failed {
                message: e.to_string(),
                transient: false,
            })?;
            let feedback = parse_feedback(&input).map_err(|message| PortError::Failed {
                message,
                transient: false,
            })?;
            submit_feedback(&self.state, &target.streamer_id, &feedback)
                .await
                .map_err(|e| port_error(&e))?
                .ok_or_else(|| PortError::Failed {
                    message: "Alert no longer exists".to_owned(),
                    transient: false,
                })
        })
    }
}
