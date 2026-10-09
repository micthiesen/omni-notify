//! The personal tools' contracts: metadata, policy and the schema types
//! their input and output schemas derive from (`omni_mcp_kit::schema`). These
//! types describe the wire format only; the handlers decode and encode with
//! their own types. After changing anything here run `cargo xtask mcp-golden`
//! and review the snapshot diff.

use std::collections::BTreeMap;

use omni_mcp_kit::schema::{Lit, Literal, NumberLiterals, one_of};
use omni_mcp_kit::{Annotations, ExecutorPolicy, Policy, ToolDef, ToolDefinition, ToolInfo};
use schemars::JsonSchema;

pub static PETS_READ: ToolDef<PetsReadInput, PetsReadOutput> = ToolDef::new(ToolInfo {
    name: "pets_read",
    title: "Read Pet Weight Data",
    description: "List pets with bounded recent weight and visit history, or read a bounded slice for one pet. This uses only Omni's local synchronized data.",
    annotations: Annotations {
        read_only_hint: true,
        destructive_hint: false,
        idempotent_hint: true,
        open_world_hint: false,
    },
    policy: Policy {
        side_effects: &[],
        cost: "No external traffic or monetary cost",
        recommended: ExecutorPolicy::Allow,
    },
});

pub static COSTS_READ: ToolDef<CostsReadInput, CostsReadOutput> = ToolDef::new(ToolInfo {
    name: "costs_read",
    title: "Read Omni Cost Telemetry",
    description: "Summarize Omni's persisted model, search, TTS, retrieval, and transcription costs for a fixed time range. Unknown-price events remain explicit.",
    annotations: Annotations {
        read_only_hint: true,
        destructive_hint: false,
        idempotent_hint: true,
        open_world_hint: false,
    },
    policy: Policy {
        side_effects: &[],
        cost: "No external traffic or monetary cost",
        recommended: ExecutorPolicy::Allow,
    },
});

/// Every personal tool, in serving order.
pub static TOOLS: [&dyn ToolDefinition; 2] = [&PETS_READ, &COSTS_READ];

#[derive(JsonSchema)]
#[schemars(rename_all = "camelCase")]
pub struct PetsReadInputList {
    #[schemars(transform = Literal("list"))]
    pub resource: Lit,
    #[schemars(range(max = 100), extend("default" = 10))]
    pub history_limit: Option<u64>,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "camelCase")]
pub struct PetsReadInputHistory {
    #[schemars(transform = Literal("history"))]
    pub resource: Lit,
    #[schemars(length(min = 1, max = 200))]
    pub pet_id: String,
    #[schemars(description = "Zero-based result offset", extend("default" = 0))]
    pub cursor: Option<u64>,
    #[schemars(range(min = 1, max = 100), extend("default" = 25))]
    pub limit: Option<u64>,
}

#[derive(JsonSchema)]
#[schemars(untagged, transform = one_of)]
pub enum PetsReadInput {
    List(PetsReadInputList),
    History(PetsReadInputHistory),
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct RecentWeight {
    pub timestamp: String,
    pub weight: f64,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct RecentVisit {
    pub date: String,
    pub count: u64,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "camelCase", deny_unknown_fields)]
pub struct Pet {
    pub pet_id: String,
    pub name: String,
    pub current_weight: f64,
    pub updated_at: String,
    pub recent_weights: Vec<RecentWeight>,
    pub recent_visits: Vec<RecentVisit>,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct PetsReadOutputList {
    #[schemars(transform = Literal("list"))]
    pub resource: Lit,
    pub pets: Vec<Pet>,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "camelCase", deny_unknown_fields)]
pub struct PetsReadOutputHistoryPet {
    pub pet_id: String,
    pub name: String,
    pub current_weight: f64,
    pub updated_at: String,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "camelCase", deny_unknown_fields)]
pub struct PetsReadOutputHistory {
    pub next_cursor: Option<u64>,
    pub total: u64,
    #[schemars(transform = Literal("history"))]
    pub resource: Lit,
    pub pet: PetsReadOutputHistoryPet,
    pub items: Vec<RecentWeight>,
}

#[derive(JsonSchema)]
#[schemars(untagged, transform = one_of)]
pub enum PetsReadOutput {
    List(PetsReadOutputList),
    History(PetsReadOutputHistory),
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct CostsReadInput {
    #[schemars(extend("default" = 30), transform = NumberLiterals(&[7, 30, 90]))]
    pub days: Option<Lit>,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct Range {
    pub days: Option<f64>,
    pub from: Option<f64>,
    pub to: f64,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "camelCase", deny_unknown_fields)]
pub struct HighestDay {
    pub date: String,
    pub cost_cents: f64,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "camelCase", deny_unknown_fields)]
pub struct Summary {
    pub selected_cost_cents: f64,
    pub all_time_cost_cents: f64,
    pub all_time_unknown_event_count: u64,
    pub average_daily_cost_cents: f64,
    pub highest_day: Option<HighestDay>,
    pub event_count: u64,
    pub unknown_event_count: u64,
    #[schemars(range(min = 0))]
    pub input_tokens: f64,
    #[schemars(range(min = 0))]
    pub output_tokens: f64,
    #[schemars(range(min = 0))]
    pub characters: f64,
    #[schemars(range(min = 0))]
    pub requests: f64,
    #[schemars(range(min = 0))]
    pub credits: f64,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "camelCase", deny_unknown_fields)]
pub struct DailyItem {
    pub date: String,
    pub cost_cents: f64,
    pub by_feature: BTreeMap<String, f64>,
    pub priced_event_count: u64,
    pub unknown_event_count: u64,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "camelCase", deny_unknown_fields)]
pub struct ByFeatureItem {
    pub feature: String,
    pub cost_cents: f64,
    pub event_count: u64,
    pub unknown_event_count: u64,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "camelCase", deny_unknown_fields)]
pub struct ByServiceItem {
    pub service: String,
    pub model: Option<String>,
    pub category: String,
    pub cost_cents: f64,
    pub event_count: u64,
    pub unknown_event_count: u64,
    #[schemars(range(min = 0))]
    pub input_tokens: f64,
    #[schemars(range(min = 0))]
    pub input_no_cache_tokens: f64,
    #[schemars(range(min = 0))]
    pub cache_read_tokens: f64,
    #[schemars(range(min = 0))]
    pub cache_write_tokens: f64,
    #[schemars(range(min = 0))]
    pub output_tokens: f64,
    #[schemars(range(min = 0))]
    pub reasoning_tokens: f64,
    #[schemars(range(min = 0))]
    pub characters: f64,
    #[schemars(range(min = 0))]
    pub requests: f64,
    #[schemars(range(min = 0))]
    pub credits: f64,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "camelCase", deny_unknown_fields)]
pub struct Usage {
    #[schemars(skip_serializing_if = "Option::is_none", range(min = 0))]
    pub input_tokens: Option<f64>,
    #[schemars(skip_serializing_if = "Option::is_none", range(min = 0))]
    pub input_no_cache_tokens: Option<f64>,
    #[schemars(skip_serializing_if = "Option::is_none", range(min = 0))]
    pub cache_read_tokens: Option<f64>,
    #[schemars(skip_serializing_if = "Option::is_none", range(min = 0))]
    pub cache_write_tokens: Option<f64>,
    #[schemars(skip_serializing_if = "Option::is_none", range(min = 0))]
    pub output_tokens: Option<f64>,
    #[schemars(skip_serializing_if = "Option::is_none", range(min = 0))]
    pub reasoning_tokens: Option<f64>,
    #[schemars(skip_serializing_if = "Option::is_none", range(min = 0))]
    pub characters: Option<f64>,
    #[schemars(skip_serializing_if = "Option::is_none", range(min = 0))]
    pub requests: Option<f64>,
    #[schemars(skip_serializing_if = "Option::is_none", range(min = 0))]
    pub credits: Option<f64>,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecentItem {
    pub event_id: String,
    pub incurred_at: f64,
    pub category: String,
    pub feature: String,
    pub operation: String,
    pub service: String,
    pub model: Option<String>,
    pub cost_cents: Option<f64>,
    pub price_status: String,
    pub usage: Usage,
    pub run_id: Option<String>,
}

#[derive(JsonSchema)]
#[schemars(rename_all = "camelCase", deny_unknown_fields)]
pub struct CostsReadOutput {
    pub range: Range,
    pub summary: Summary,
    pub daily: Vec<DailyItem>,
    pub by_feature: Vec<ByFeatureItem>,
    pub by_service: Vec<ByServiceItem>,
    pub recent: Vec<RecentItem>,
}
