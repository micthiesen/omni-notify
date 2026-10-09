//! `/api/costs`: the cost summary.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::common::Ms;

/// `?days=7|30|90|all`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CostRange {
    Days(u32),
    All,
}

impl CostRange {
    /// `days` as the response carries it (`null` for all time).
    pub fn days(self) -> Option<u32> {
        match self {
            CostRange::Days(days) => Some(days),
            CostRange::All => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CostCategory {
    #[serde(rename = "llm")]
    Llm,
    #[serde(rename = "search")]
    Search,
    #[serde(rename = "tts")]
    Tts,
    #[serde(rename = "retrieval")]
    Retrieval,
    #[serde(rename = "transcription")]
    Transcription,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CostPriceStatus {
    #[serde(rename = "priced")]
    Priced,
    #[serde(rename = "estimated")]
    Estimated,
    #[serde(rename = "free")]
    Free,
    #[serde(rename = "unknown")]
    Unknown,
}

/// `CostUsage`: every counter optional.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostUsage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_no_cache_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub characters: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requests: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credits: Option<f64>,
}

/// `Required<CostUsage>` totals.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostUsageTotals {
    pub input_tokens: f64,
    pub input_no_cache_tokens: f64,
    pub cache_read_tokens: f64,
    pub cache_write_tokens: f64,
    pub output_tokens: f64,
    pub reasoning_tokens: f64,
    pub characters: f64,
    pub requests: f64,
    pub credits: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CostsRangeInfo {
    pub days: Option<u32>,
    pub from: Option<Ms>,
    pub to: Ms,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostDayTotal {
    pub date: String,
    pub cost_cents: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostsSummary {
    pub selected_cost_cents: f64,
    pub all_time_cost_cents: f64,
    pub all_time_unknown_event_count: u64,
    pub average_daily_cost_cents: f64,
    pub highest_day: Option<CostDayTotal>,
    pub event_count: u64,
    pub unknown_event_count: u64,
    pub input_tokens: f64,
    pub output_tokens: f64,
    pub characters: f64,
    pub requests: f64,
    pub credits: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostDailyRow {
    /// `YYYY-MM-DD` in the configured `TZ`.
    pub date: String,
    pub cost_cents: f64,
    pub by_feature: BTreeMap<String, f64>,
    pub priced_event_count: u64,
    pub unknown_event_count: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostFeatureRow {
    pub feature: String,
    pub cost_cents: f64,
    pub event_count: u64,
    pub unknown_event_count: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostServiceRow {
    pub service: String,
    pub model: Option<String>,
    pub category: CostCategory,
    pub cost_cents: f64,
    pub event_count: u64,
    pub unknown_event_count: u64,
    #[serde(flatten)]
    pub usage: CostUsageTotals,
}

/// One recent event; `model` and `runId` are explicit `null`s when absent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostRecentEvent {
    pub event_id: String,
    pub incurred_at: Ms,
    pub category: CostCategory,
    pub feature: String,
    pub operation: String,
    pub service: String,
    pub model: Option<String>,
    pub cost_cents: Option<f64>,
    pub price_status: CostPriceStatus,
    pub usage: CostUsage,
    pub run_id: Option<String>,
}

/// `GET /api/costs` body.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostsResponse {
    pub range: CostsRangeInfo,
    pub summary: CostsSummary,
    pub daily: Vec<CostDailyRow>,
    pub by_feature: Vec<CostFeatureRow>,
    pub by_service: Vec<CostServiceRow>,
    pub recent: Vec<CostRecentEvent>,
}
