//! Briefing notification history (`GET /api/briefings` and the
//! `BriefingsReader` port payload that the MCP `briefings_list` tool consumes).

use serde::{Deserialize, Serialize};

use crate::common::Ms;

/// One delivered briefing notification as the API serves it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BriefingNotification {
    pub title: String,
    pub message: String,
    pub url: String,
    pub timestamp: Ms,
    /// Task-run id that produced the notification (`null` for older rows).
    pub run_id: Option<String>,
    /// LLM cost in USD cents; `null` when unpriced or never computed.
    pub cost_cents: Option<f64>,
}

/// One briefing's notifications, newest first (`GET /api/briefings`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BriefingSummary {
    pub name: String,
    pub notifications: Vec<BriefingNotification>,
}

/// `GET /api/briefings`: briefings ordered by their newest notification.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BriefingsResponse {
    pub briefings: Vec<BriefingSummary>,
}

/// One `briefing-history` row as the `BriefingsReader` port hands it over:
/// notifications newest first (histories are ordered by their newest
/// notification), with absent `runId` / `costCents` normalized to `null`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BriefingHistory {
    pub briefing_name: String,
    pub notifications: Vec<BriefingNotification>,
}

/// Path of the briefings route.
pub mod paths {
    pub const BRIEFINGS: &str = "/api/briefings";
}
