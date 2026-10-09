//! Port of `src/claude-resets/policy.spec.ts` (all cases kept).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use jiff::tz::TimeZone;
use omni_core::js::utf16_len;
use omni_personal::claude_resets::policy::select_claude_reset_alerts;
use omni_personal::claude_resets::source::{CatalogEvent, CatalogSource, ClaudeResetSource};

fn tz() -> TimeZone {
    TimeZone::get("America/Vancouver").unwrap()
}

fn now() -> i64 {
    "2026-10-07T16:00:00Z"
        .parse::<jiff::Timestamp>()
        .unwrap()
        .as_millisecond()
}

fn src(url: &str) -> CatalogSource {
    CatalogSource { url: url.into() }
}

fn event() -> CatalogEvent {
    CatalogEvent {
        id: "october-reset".into(),
        date: "2026-10-07T15:00:00Z".into(),
        kind: "counter-reset".into(),
        status: "historic".into(),
        confidence: "confirmed".into(),
        plans: vec!["all".into()],
        surfaces: vec!["claude-code".into(), "claude-app".into()],
        title: "Usage counters reset".into(),
        summary: "The tracker reports a reset.".into(),
        sources: vec![src("https://x.com/ClaudeDevs/status/123")],
    }
}

fn source(events: Vec<CatalogEvent>) -> ClaudeResetSource {
    ClaudeResetSource {
        updated: "2026-10-07".into(),
        events,
    }
}

#[test]
fn reports_a_reset_without_claiming_the_users_account_was_verified() {
    let alerts = select_claude_reset_alerts(&source(vec![event()]), now(), &tz());
    let alert = &alerts[0];
    assert_eq!(alert.title, "Claude Code reset reported");
    assert!(alert.message.contains("Your account has not been verified"));
    assert!(alert.message.contains("Source: Reset Radar"));
    assert_eq!(alert.url, event().sources[0].url);
}

#[test]
fn preserves_banked_wording_without_inventing_an_automatic_refill_or_expiry() {
    let alerts = select_claude_reset_alerts(
        &source(vec![CatalogEvent {
            title: "A reset to bank and spend later".into(),
            summary: "Subscribers can save this reset until they choose to redeem it.".into(),
            ..event()
        }]),
        now(),
        &tz(),
    );
    let message = &alerts[0].message;
    assert!(message.contains("bank and spend later"));
    assert!(message.contains("Settings → Usage"));
    assert!(message.contains("Banked resets refill usage only when redeemed"));
    assert!(!message.contains("Non-banked"));
    assert!(!message.contains("Oct 22"));
}

#[test]
fn fails_closed_for_policies_projections_upcoming_and_unconfirmed_reports() {
    let patches: Vec<fn(&mut CatalogEvent)> = vec![
        |e| e.kind = "policy-change".into(),
        |e| e.kind = "unknown".into(),
        |e| e.status = "projected".into(),
        |e| e.status = "upcoming".into(),
        |e| e.confidence = "uncertain".into(),
        |e| e.confidence = "projected".into(),
        |e| e.surfaces = vec!["claude-app".into()],
    ];
    for patch in patches {
        let mut e = event();
        patch(&mut e);
        assert!(select_claude_reset_alerts(&source(vec![e]), now(), &tz()).is_empty());
    }
}

#[test]
fn uses_event_age_never_catalog_update_or_revision_to_bound_replay() {
    for date in [
        "2026-10-05T15:59:59Z",
        "2026-10-07T16:05:01Z",
        "2026-09-22T16:31:00Z",
    ] {
        let e = CatalogEvent {
            date: date.into(),
            ..event()
        };
        assert!(select_claude_reset_alerts(&source(vec![e]), now(), &tz()).is_empty());
    }
    let edge = CatalogEvent {
        date: "2026-10-05T16:00:00Z".into(),
        ..event()
    };
    assert_eq!(
        select_claude_reset_alerts(&source(vec![edge]), now(), &tz()).len(),
        1
    );
}

#[test]
fn keeps_stable_event_and_post_identities_through_cosmetic_revisions() {
    let original = select_claude_reset_alerts(&source(vec![event()]), now(), &tz()).remove(0);
    let revised_event = CatalogEvent {
        title: "Revised wording".into(),
        sources: vec![src(
            "https://www.twitter.com/newhandle/status/123/photo/1?s=20#content",
        )],
        ..event()
    };
    let revised =
        select_claude_reset_alerts(&source(vec![revised_event.clone()]), now(), &tz()).remove(0);
    assert_eq!(revised.key, original.key);
    assert_eq!(revised.aliases, original.aliases);
    let renamed = CatalogEvent {
        id: "new-catalog-id".into(),
        ..revised_event
    };
    assert_eq!(
        select_claude_reset_alerts(&source(vec![event(), renamed]), now(), &tz()).len(),
        1
    );
}

#[test]
fn does_not_deduplicate_unrelated_events_sharing_a_background_source() {
    let shared = src("https://www.anthropic.com/news");
    let first = CatalogEvent {
        sources: vec![event().sources[0].clone(), shared.clone()],
        ..event()
    };
    let second = CatalogEvent {
        id: "different".into(),
        sources: vec![src("https://x.com/ClaudeDevs/status/456"), shared],
        ..event()
    };
    assert_eq!(
        select_claude_reset_alerts(&source(vec![first, second]), now(), &tz()).len(),
        2
    );
}

#[test]
fn keeps_source_query_and_fragment_identities_and_falls_back_to_tracker_details() {
    let alerts = select_claude_reset_alerts(
        &source(vec![
            CatalogEvent {
                sources: vec![src("https://example.com/?id=1#post")],
                ..event()
            },
            CatalogEvent {
                id: "second".into(),
                sources: vec![src("https://example.com/?id=2#post")],
                ..event()
            },
            CatalogEvent {
                id: "third".into(),
                sources: vec![],
                ..event()
            },
        ]),
        now(),
        &tz(),
    );
    assert_eq!(alerts.len(), 3);
    let third = alerts.iter().find(|a| a.key == "third:reported").unwrap();
    assert_eq!(third.url, "https://resetradar.com/#third");
}

#[test]
fn bounds_noisy_summaries_and_removes_urls_from_the_preview() {
    let alerts = select_claude_reset_alerts(
        &source(vec![CatalogEvent {
            summary: format!("See https://example.com {}", "word ".repeat(1_000)),
            ..event()
        }]),
        now(),
        &tz(),
    );
    assert!(!alerts[0].message.contains("https://"));
    let second_line = alerts[0].message.split('\n').nth(1).unwrap();
    assert!(utf16_len(second_line) <= 200);
}
