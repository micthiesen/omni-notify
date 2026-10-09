//! `GET /api/streamers/:id/intelligence-details` and
//! `POST /api/streamers/:id/intelligence-feedback` (`server.ts` 779-848).

use std::sync::{Arc, OnceLock};

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, post};
use omni_core::clock::SharedClock;
use omni_runtime::Ports;
use omni_server_kit::{ApiError, JsonBody, json_response};
use omni_store::{Store, StoreError};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::persistence::{get_diagnostics, get_events, get_intelligence, record_feedback};
use crate::service::LivestreamIntelligenceService;
use crate::types::LivestreamFeedbackVerdict;

const DEFAULT_EVENT_LIMIT: usize = 100;
const NOTE_MAX_CHARS: usize = 500;

/// Shared by the routes and the port.
#[derive(Clone)]
pub struct IntelState {
    pub store: Store,
    pub clock: SharedClock,
    pub ports: Ports,
    /// Set by the boot step when `LIVESTREAM_INTELLIGENCE_ENABLED`.
    pub service: Arc<OnceLock<LivestreamIntelligenceService>>,
}

#[derive(Debug, thiserror::Error)]
pub enum DetailsError {
    #[error("{0}")]
    Store(#[from] StoreError),
    #[error("{0}")]
    Encode(#[from] serde_json::Error),
}

impl IntelState {
    pub fn service(&self) -> Option<&LivestreamIntelligenceService> {
        self.service.get()
    }

    /// Whether the live directory knows the streamer (`Unknown streamer` otherwise).
    pub async fn streamer_exists(&self, id: &str) -> bool {
        let Some(directory) = self.ports.live_directory() else {
            return false;
        };
        match directory.streamers().await {
            Ok(streamers) => streamers
                .iter()
                .any(|s| s.get("id").and_then(Value::as_str) == Some(id)),
            Err(error) => {
                tracing::warn!(target: crate::LOG, %error, "Live directory unavailable");
                false
            }
        }
    }

    /// The runtime diagnostics, or `null` while the service is disabled.
    pub async fn runtime_json(&self) -> Result<Value, DetailsError> {
        match self.service() {
            Some(service) => Ok(serde_json::to_value(service.runtime_diagnostics().await?)?),
            None => Ok(Value::Null),
        }
    }

    /// `{intelligence, diagnostics, events, runtime, generatedAt}`.
    pub async fn details_json(&self, id: &str, limit: usize) -> Result<Value, DetailsError> {
        let intelligence = get_intelligence(&self.store, id).await?;
        let diagnostics = get_diagnostics(&self.store, id).await?;
        let events = get_events(&self.store, Some(id), limit).await?;
        Ok(json!({
            "intelligence": serde_json::to_value(intelligence)?,
            "diagnostics": serde_json::to_value(diagnostics)?,
            "events": serde_json::to_value(events)?,
            "runtime": self.runtime_json().await?,
            "generatedAt": self.clock.now_ms(),
        }))
    }
}

/// JS-formatted JSON (integral numbers without `.0`).
pub fn js_json(status: StatusCode, value: &Value) -> Response {
    json_response(status, omni_core::js::json_stringify(value))
}

#[derive(Debug, Deserialize)]
struct DetailsQuery {
    limit: Option<String>,
}

/// `Number(limit)`: a positive integer, else 100.
fn parse_limit(raw: Option<&str>) -> usize {
    let n = raw.map_or(f64::NAN, omni_core::js::string_to_number);
    if n.is_finite() && n.fract() == 0.0 && n > 0.0 {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let limit = n.min(usize::MAX as f64) as usize;
        limit
    } else {
        DEFAULT_EVENT_LIMIT
    }
}

async fn details(
    State(state): State<IntelState>,
    Path(id): Path<String>,
    Query(query): Query<DetailsQuery>,
) -> Result<Response, ApiError> {
    if !state.streamer_exists(&id).await {
        return Err(ApiError::not_found("Unknown streamer"));
    }
    let limit = parse_limit(query.limit.as_deref());
    let body = state
        .details_json(&id, limit)
        .await
        .map_err(ApiError::internal)?;
    Ok(js_json(StatusCode::OK, &body))
}

/// Effect `Schema.isUUID()`: an RFC 9562 UUID (version 1-8, variant 10xx), or
/// the nil or max UUID, in either case.
fn is_uuid(value: &str) -> bool {
    const NIL: &str = "00000000-0000-0000-0000-000000000000";
    const MAX: &str = "ffffffff-ffff-ffff-ffff-ffffffffffff";
    if value == NIL || value.eq_ignore_ascii_case(MAX) {
        return true;
    }
    let groups: Vec<&str> = value.split('-').collect();
    let shaped = groups.len() == 5
        && groups
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(group, len)| group.len() == len && group.chars().all(|c| c.is_ascii_hexdigit()));
    shaped
        && groups[2].starts_with(['1', '2', '3', '4', '5', '6', '7', '8'])
        && groups[3].starts_with(['8', '9', 'a', 'b', 'A', 'B'])
}

/// A validated feedback body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeedbackInput {
    pub alert_id: String,
    pub verdict: LivestreamFeedbackVerdict,
    pub note: Option<String>,
}

/// `livestreamFeedbackSchema`: `alertId` UUID, `verdict` literal, `note` <= 500 chars.
pub fn parse_feedback(body: &Value) -> Result<FeedbackInput, String> {
    let Some(object) = body.as_object() else {
        return Err("Expected an object".to_owned());
    };
    let alert_id = match object.get("alertId") {
        Some(Value::String(id)) if is_uuid(id) => id.clone(),
        Some(Value::String(_)) => return Err("alertId must be a UUID".to_owned()),
        _ => return Err("alertId is required and must be a string".to_owned()),
    };
    let verdict = object
        .get("verdict")
        .and_then(Value::as_str)
        .and_then(LivestreamFeedbackVerdict::parse)
        .ok_or_else(|| {
            "verdict must be one of \"useful\", \"not_useful\", \"false_positive\"".to_owned()
        })?;
    let note = match object.get("note") {
        None => None,
        Some(Value::String(note)) if omni_core::js::utf16_len(note) <= NOTE_MAX_CHARS => {
            Some(note.clone())
        }
        Some(Value::String(_)) => {
            return Err(format!("note must be at most {NOTE_MAX_CHARS} characters"));
        }
        Some(_) => return Err("note must be a string".to_owned()),
    };
    Ok(FeedbackInput {
        alert_id,
        verdict,
        note,
    })
}

/// Records feedback for the streamer's latest alert; `None` when it no longer exists.
pub async fn submit_feedback(
    state: &IntelState,
    streamer_id: &str,
    input: &FeedbackInput,
) -> Result<Option<Value>, DetailsError> {
    let feedback = record_feedback(
        &state.store,
        streamer_id,
        &input.alert_id,
        input.verdict,
        input.note.as_deref(),
    )
    .await?;
    feedback
        .map(serde_json::to_value)
        .transpose()
        .map_err(Into::into)
}

async fn feedback(
    State(state): State<IntelState>,
    Path(id): Path<String>,
    JsonBody(body): JsonBody<Value>,
) -> Result<Response, ApiError> {
    if !state.streamer_exists(&id).await {
        return Err(ApiError::not_found("Unknown streamer"));
    }
    let input = parse_feedback(&body).map_err(ApiError::bad_request)?;
    match submit_feedback(&state, &id, &input)
        .await
        .map_err(ApiError::internal)?
    {
        Some(feedback) => Ok(js_json(
            StatusCode::CREATED,
            &json!({ "feedback": feedback }),
        )),
        None => Err(ApiError::not_found("Alert no longer exists")),
    }
}

pub fn router(state: IntelState) -> Router {
    Router::new()
        .route("/api/streamers/{id}/intelligence-details", get(details))
        .route("/api/streamers/{id}/intelligence-feedback", post(feedback))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limit_follows_number_parsing() {
        assert_eq!(parse_limit(None), 100);
        assert_eq!(parse_limit(Some("")), 100);
        assert_eq!(parse_limit(Some("5")), 5);
        assert_eq!(parse_limit(Some(" 7 ")), 7);
        assert_eq!(parse_limit(Some("2.5")), 100);
        assert_eq!(parse_limit(Some("-1")), 100);
        assert_eq!(parse_limit(Some("500")), 500);
    }

    #[test]
    fn feedback_body_is_validated() {
        let ok = json!({"alertId": "0b9f8c56-1c7c-4a43-9b8e-2f43c1a0d3e1", "verdict": "useful"});
        assert!(parse_feedback(&ok).is_ok());
        assert!(parse_feedback(&json!({"alertId": "x", "verdict": "useful"})).is_err());
        assert!(
            parse_feedback(
                &json!({"alertId": "0b9f8c56-1c7c-4a43-9b8e-2f43c1a0d3e1", "verdict": "meh"})
            )
            .is_err()
        );
        assert!(parse_feedback(&Value::Null).is_err());
    }

    #[test]
    fn uuids_follow_the_effect_schema_pattern() {
        assert!(is_uuid("0B9F8C56-1C7C-4A43-9B8E-2F43C1A0D3E1"));
        assert!(is_uuid("00000000-0000-0000-0000-000000000000"));
        assert!(is_uuid("FFFFFFFF-ffff-FFFF-ffff-FFFFFFFFFFFF"));
        // Version nibble 0 and 9 are rejected, as is a non-RFC variant.
        assert!(!is_uuid("0b9f8c56-1c7c-0a43-9b8e-2f43c1a0d3e1"));
        assert!(!is_uuid("0b9f8c56-1c7c-9a43-9b8e-2f43c1a0d3e1"));
        assert!(!is_uuid("0b9f8c56-1c7c-4a43-7b8e-2f43c1a0d3e1"));
        assert!(!is_uuid("0b9f8c561c7c4a439b8e2f43c1a0d3e1"));
    }
}
