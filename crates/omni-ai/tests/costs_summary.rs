//! Port of `src/costs/summary.spec.ts`.
#![allow(clippy::unwrap_used)]

use jiff::tz::TimeZone;
use omni_ai::costs::{CostEventData, summarize};
use omni_api::costs::{CostCategory, CostPriceStatus, CostRange, CostUsage};
use omni_store::cbor::Extra;

const DAY: i64 = 86_400_000;

fn utc(y: i16, m: i8, d: i8, h: i8, min: i8) -> i64 {
    jiff::civil::date(y, m, d)
        .at(h, min, 0, 0)
        .to_zoned(TimeZone::UTC)
        .unwrap()
        .timestamp()
        .as_millisecond()
}

fn event(id: &str, at: i64) -> CostEventData {
    CostEventData {
        category: CostCategory::Llm,
        feature: "briefings".to_owned(),
        operation: "generate".to_owned(),
        service: "google".to_owned(),
        model: Some("gemini-test".to_owned()),
        cost_cents: Some(1.0),
        price_status: CostPriceStatus::Estimated,
        usage: CostUsage {
            input_tokens: Some(100.0),
            output_tokens: Some(20.0),
            requests: Some(1.0),
            ..CostUsage::default()
        },
        event_id: id.to_owned(),
        incurred_at: at,
        run_id: None,
        extra: Extra::new(),
    }
}

fn with_cost(mut e: CostEventData, cents: Option<f64>) -> CostEventData {
    e.cost_cents = cents;
    e
}

fn now() -> i64 {
    utc(2026, 7, 20, 12, 0)
}

fn vancouver() -> TimeZone {
    TimeZone::get("America/Vancouver").unwrap()
}

#[test]
fn filters_the_selected_range_while_retaining_an_all_time_total() {
    let now = now();
    let result = summarize(
        &[
            with_cost(event("old", now - 40 * DAY), Some(5.0)),
            with_cost(event("recent", now - 2 * DAY), Some(2.0)),
            with_cost(event("future", now + 1), Some(100.0)),
        ],
        CostRange::Days(30),
        now,
        &TimeZone::UTC,
    );
    assert_eq!(result.summary.selected_cost_cents, 2.0);
    assert_eq!(result.summary.all_time_cost_cents, 7.0);
    assert_eq!(result.summary.event_count, 1);
    assert!((result.summary.average_daily_cost_cents - 2.0 / 30.0).abs() < 1e-9);
}

#[test]
fn groups_fractional_costs_and_preserves_unknown_usage() {
    let now = now();
    let mut unknown = event("b", now - 500);
    unknown.category = CostCategory::Retrieval;
    unknown.feature = "press-pods".to_owned();
    unknown.service = "jina".to_owned();
    unknown.model = None;
    unknown.cost_cents = None;
    unknown.price_status = CostPriceStatus::Unknown;
    unknown.usage = CostUsage {
        requests: Some(1.0),
        ..CostUsage::default()
    };
    let result = summarize(
        &[with_cost(event("a", now - 1000), Some(0.25)), unknown],
        CostRange::All,
        now,
        &vancouver(),
    );
    assert_eq!(result.summary.selected_cost_cents, 0.25);
    assert_eq!(result.summary.unknown_event_count, 1);
    assert_eq!(result.summary.all_time_unknown_event_count, 1);
    assert_eq!(result.summary.requests, 2.0);
    let by_feature = serde_json::to_value(&result.by_feature).unwrap();
    assert_eq!(
        by_feature,
        serde_json::json!([
            { "feature": "briefings", "costCents": 0.25, "eventCount": 1, "unknownEventCount": 0 },
            { "feature": "press-pods", "costCents": 0.0, "eventCount": 1, "unknownEventCount": 1 },
        ])
    );
    let recent = serde_json::to_value(&result.recent[0]).unwrap();
    assert_eq!(recent["eventId"], "b");
    assert_eq!(recent["model"], serde_json::Value::Null);
    assert_eq!(recent["runId"], serde_json::Value::Null);
}

#[test]
fn only_sums_usage_fields_when_multiple_events_share_a_service_row() {
    let now = now();
    let mut a = event("a", now - 1000);
    a.model = None;
    let mut b = event("b", now - 500);
    b.model = None;
    let result = summarize(&[a, b], CostRange::All, now, &TimeZone::UTC);
    assert_eq!(result.by_service.len(), 1);
    let row = serde_json::to_value(&result.by_service[0]).unwrap();
    assert_eq!(row["service"], "google");
    assert_eq!(row["model"], serde_json::Value::Null);
    assert_eq!(row["category"], "llm");
    assert_eq!(row["eventCount"], 2);
    assert_eq!(row["inputTokens"], 200.0);
    assert_eq!(row["outputTokens"], 40.0);
    assert_eq!(row["requests"], 2.0);
}

#[test]
fn does_not_call_an_unknown_only_date_the_highest_priced_day() {
    let now = now();
    let mut unknown = with_cost(event("unknown", now), None);
    unknown.price_status = CostPriceStatus::Unknown;
    let result = summarize(&[unknown], CostRange::Days(7), now, &TimeZone::UTC);
    assert_eq!(result.summary.highest_day, None);
    assert_eq!(result.daily[0].priced_event_count, 0);
    assert_eq!(result.daily[0].unknown_event_count, 1);
}

#[test]
fn uses_the_configured_timezone_for_day_boundaries() {
    let before_midnight = utc(2026, 7, 20, 6, 59);
    let after_midnight = utc(2026, 7, 20, 7, 1);
    let result = summarize(
        &[event("a", before_midnight), event("b", after_midnight)],
        CostRange::All,
        now(),
        &vancouver(),
    );
    let dates: Vec<&str> = result.daily.iter().map(|d| d.date.as_str()).collect();
    assert_eq!(dates, ["2026-07-19", "2026-07-20"]);
}
