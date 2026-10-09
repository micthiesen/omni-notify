//! Reset Radar's structured catalog.

use jiff::tz::TimeZone;
use omni_http::public::PublicHttpClient;
use serde::Deserialize;
use serde_json::Value;

use crate::reset_alerts::source::{Check, ResetSourceError, decode, read_json};

pub const CATALOG_URL: &str = "https://resetradar.com/data/events.json";
const MAX_EVENTS: usize = 5_000;
const MAX_LABELS: usize = 50;

/// `ClaudeResetSourceSchema`; unrelated catalog metadata is discarded.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ClaudeResetSource {
    pub updated: String,
    pub events: Vec<CatalogEvent>,
}

/// Unknown classifications are retained so selection can fail closed.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct CatalogEvent {
    pub id: String,
    pub date: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub status: String,
    pub confidence: String,
    pub plans: Vec<String>,
    pub surfaces: Vec<String>,
    pub title: String,
    pub summary: String,
    pub sources: Vec<CatalogSource>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct CatalogSource {
    pub url: String,
}

fn check_source(source: &ClaudeResetSource, tz: &TimeZone) -> Result<(), String> {
    let c = Check { tz };
    c.timestamp("updated", &source.updated)?;
    c.max_items("events", source.events.len(), MAX_EVENTS)?;
    for (i, event) in source.events.iter().enumerate() {
        let p = |field: &str| format!("events[{i}].{field}");
        c.identifier(&p("id"), &event.id)?;
        c.timestamp(&p("date"), &event.date)?;
        c.identifier(&p("type"), &event.kind)?;
        c.identifier(&p("status"), &event.status)?;
        c.identifier(&p("confidence"), &event.confidence)?;
        c.max_items(&p("plans"), event.plans.len(), MAX_LABELS)?;
        for plan in &event.plans {
            c.identifier(&p("plans[]"), plan)?;
        }
        c.max_items(&p("surfaces"), event.surfaces.len(), MAX_LABELS)?;
        for surface in &event.surfaces {
            c.identifier(&p("surfaces[]"), surface)?;
        }
        c.text(&p("title"), &event.title)?;
        c.text(&p("summary"), &event.summary)?;
        c.max_items(&p("sources"), event.sources.len(), MAX_LABELS)?;
        for source in &event.sources {
            c.url(&p("sources[].url"), &source.url)?;
        }
    }
    Ok(())
}

/// Decodes and validates a catalog document.
pub fn decode_claude_source(value: Value, tz: &TimeZone) -> Result<ClaudeResetSource, String> {
    decode(value, |source| check_source(source, tz))
}

/// Reads the catalog.
pub async fn read_claude_reset_source(
    http: &PublicHttpClient,
    tz: &TimeZone,
) -> Result<ClaudeResetSource, ResetSourceError> {
    let raw = read_json(http, CATALOG_URL).await?;
    decode_claude_source(raw, tz)
        .map_err(|e| ResetSourceError::new(format!("read {CATALOG_URL}"), e))
}
