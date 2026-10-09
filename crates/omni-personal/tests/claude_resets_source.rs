//! Reset Radar catalog decoding and bounded reads.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use jiff::tz::TimeZone;
use omni_personal::claude_resets::source::decode_claude_source;
use serde_json::{Value, json};

fn tz() -> TimeZone {
    TimeZone::get("America/Vancouver").unwrap()
}

fn event() -> Value {
    json!({
        "id": "2026-09-22-saved-reset",
        "date": "2026-09-22T16:31:00Z",
        "type": "counter-reset",
        "status": "historic",
        "confidence": "confirmed",
        "plans": ["all"],
        "surfaces": ["claude-code"],
        "title": "A saved reset",
        "summary": "A saved reset was announced.",
        "sources": [{"url": "https://www.anthropic.com/claude-opus-5-5"}],
    })
}

fn source(events: Vec<Value>) -> Value {
    json!({"updated": "2026-10-07", "events": events})
}

fn patched(patch: Value) -> Value {
    let mut e = event();
    for (k, v) in patch.as_object().unwrap() {
        e[k] = v.clone();
    }
    e
}

#[test]
fn reads_the_live_catalog_shape_and_discards_unrelated_metadata() {
    let mut doc = source(vec![event()]);
    doc["version"] = json!("1.0.0");
    let result = decode_claude_source(doc, &tz()).unwrap();
    assert_eq!(result.events.len(), 1);
    let e = &result.events[0];
    assert_eq!(e.id, "2026-09-22-saved-reset");
    assert_eq!(e.kind, "counter-reset");
    assert_eq!(e.surfaces, vec!["claude-code".to_owned()]);
    assert_eq!(
        e.sources[0].url,
        "https://www.anthropic.com/claude-opus-5-5"
    );
}

#[test]
fn retains_unknown_classifications_for_fail_closed_selection() {
    let decoded =
        decode_claude_source(source(vec![patched(json!({"type": "future-kind"}))]), &tz()).unwrap();
    assert_eq!(decoded.events.len(), 1);
}

#[test]
fn rejects_missing_catalog_data() {
    assert!(decode_claude_source(json!({"updated": "2026-10-07"}), &tz()).is_err());
}

#[test]
fn rejects_invalid_times_unsafe_urls_and_oversized_fields() {
    let first_source = event()["sources"][0].clone();
    for patch in [
        json!({"date": "yesterday"}),
        json!({"sources": [{"url": "http://example.com"}]}),
        json!({"sources": [{"url": "https://user:password@example.com"}]}),
        json!({"id": "x".repeat(257)}),
        json!({"summary": "x".repeat(16_385)}),
        json!({"plans": vec!["all"; 51]}),
        json!({"sources": vec![first_source.clone(); 51]}),
    ] {
        assert!(
            decode_claude_source(source(vec![patched(patch.clone())]), &tz()).is_err(),
            "{patch}"
        );
    }
}

#[test]
fn caps_the_catalog_size() {
    assert!(decode_claude_source(source(vec![event(); 5_001]), &tz()).is_err());
}
