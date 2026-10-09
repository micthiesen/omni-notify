//! `omni_runtime::ports::LiveIntelligence` for WP04 (task hooks, routes) and
//! WP12 (`livestream_get`).
//!
//! WP04 calls `observe_live` for every streamer it polled live this tick (the
//! went-live edge and each still-live poll; background streamers only on their
//! due ticks), `on_transition` with `live: false` for `observeOffline`, and
//! `after_tick` once per tick for voice-target scheduling.

use futures::future::BoxFuture;
use omni_runtime::ports::{
    LiveIntelligence, LiveObservation as PortObservation, LiveTransition, PortError,
};
use serde::Deserialize;
use serde_json::Value;

use crate::LOG;
use crate::observation::{LiveObservation, LiveStatus, Streamer};
use crate::routes::{DetailsError, IntelState, parse_feedback, submit_feedback};
use crate::service::LivestreamIntelligenceService;

fn port_error(error: &DetailsError) -> PortError {
    PortError::Failed {
        message: error.to_string(),
        transient: true,
    }
}

/// The port implementation.
pub struct IntelligencePort {
    state: IntelState,
}

impl IntelligencePort {
    pub fn new(state: IntelState) -> Self {
        Self { state }
    }

    async fn offline(&self, service: &LivestreamIntelligenceService, id: &str) {
        if let Err(error) = service.observe_offline(id).await {
            tracing::error!(target: LOG, "Intelligence offline handling failed for {id}: {error}");
        }
    }
}

/// The intelligence view of a port observation (the `LiveDirectory` DTOs).
fn decode(observation: &PortObservation) -> Result<LiveObservation, serde_json::Error> {
    Ok(LiveObservation {
        streamer: Streamer::deserialize(&observation.streamer)?,
        status: LiveStatus::deserialize(&observation.status)?,
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FeedbackPortInput {
    streamer_id: String,
}

impl LiveIntelligence for IntelligencePort {
    fn observe_live<'a>(&'a self, observation: &'a PortObservation) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let Some(service) = self.state.service() else {
                return;
            };
            let observation = match decode(observation) {
                Ok(observation) => observation,
                Err(error) => {
                    tracing::warn!(target: LOG, %error, "Skipping undecodable live observation");
                    return;
                }
            };
            let name = observation.streamer.display_name.clone();
            if let Err(error) = service.observe_live(observation).await {
                tracing::error!(target: LOG, "Intelligence observation failed for {name}: {error}");
            }
        })
    }

    fn after_tick(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let Some(service) = self.state.service() else {
                return;
            };
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
