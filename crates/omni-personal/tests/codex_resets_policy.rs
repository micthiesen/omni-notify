//! Codex reset alert policy.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use jiff::tz::TimeZone;
use omni_core::js::{to_iso_string, utf16_len};
use omni_personal::codex_resets::policy::{FEED_MAX_AGE_MS, select_reset_alerts};
use omni_personal::codex_resets::source::{
    AlertFeed, FeedItem, HistoryEvent, HistorySource, ResetHistory, decode_alert_feed,
};
use omni_personal::reset_alerts::presentation::ALERT_LOOKBACK_MS;
use serde_json::json;

fn tz() -> TimeZone {
    TimeZone::get("America/Vancouver").unwrap()
}

fn ms(s: &str) -> i64 {
    s.parse::<jiff::Timestamp>().unwrap().as_millisecond()
}

fn now() -> i64 {
    ms("2026-10-02T16:00:00Z")
}

fn item() -> FeedItem {
    FeedItem {
        id: "announcement:revision-1".into(),
        event_id: "october-reset".into(),
        post_id: Some("123".into()),
        topic: "schedule".into(),
        state: "official_scheduled".into(),
        title: "Reset scheduled".into(),
        summary: "An official post names a reset time.".into(),
        source_url: "https://x.com/thsottiaux/status/123".into(),
        evidence_id: Some("capture-123".into()),
        target_at: Some("2026-10-02T17:00:00Z".into()),
        published_at: "2026-10-02T15:00:00Z".into(),
        source_published_at: None,
        withdrawn: false,
    }
}

fn event() -> HistoryEvent {
    HistoryEvent {
        id: "123".into(),
        kind: "special_global".into(),
        scope: "all".into(),
        event_kind: "scheduled".into(),
        evidence_class: "named_public_source".into(),
        status: "active".into(),
        fulfilled_by: None,
        superseded_by: None,
        announced_at: None,
        summary: None,
        sources: vec![HistorySource {
            announcement_id: "123".into(),
            url: item().source_url,
        }],
    }
}

fn feed(items: Vec<FeedItem>) -> AlertFeed {
    AlertFeed {
        generated_at: to_iso_string(now()),
        items,
    }
}

fn history(entry: HistoryEvent) -> ResetHistory {
    ResetHistory { items: vec![entry] }
}

fn with(topic: &str, state: &str) -> FeedItem {
    FeedItem {
        topic: topic.into(),
        state: state.into(),
        ..item()
    }
}

fn completed() -> HistoryEvent {
    HistoryEvent {
        event_kind: "completed".into(),
        status: "completed".into(),
        ..event()
    }
}

#[test]
fn distinguishes_forecast_schedule_observed_rollout_and_confirmed_landing() {
    for (topic, state, expected) in [
        ("likely", "likely_forecast", "looks likely"),
        ("schedule", "official_scheduled", "announced"),
        ("rollout", "rollout_observed", "rolling out"),
        ("action", "action_claimed", "landed"),
    ] {
        let entry = if topic == "action" {
            completed()
        } else {
            event()
        };
        let alerts = select_reset_alerts(
            &feed(vec![with(topic, state)]),
            &history(entry),
            now(),
            &tz(),
        );
        let alert = &alerts[0];
        assert!(alert.title.contains(expected), "{}", alert.title);
        assert!(alert.message.contains("Non-banked reset; all users"));
        assert_eq!(alert.url, item().source_url);
        assert!(
            alert
                .message
                .contains("Source: @thsottiaux via Reset Beacon")
        );
        assert!(!alert.message.contains("https://"));
    }
}

#[test]
fn never_turns_an_elapsed_scheduled_time_into_a_landed_claim() {
    let alerts = select_reset_alerts(
        &feed(vec![FeedItem {
            target_at: Some("2026-10-02T14:00:00Z".into()),
            ..item()
        }]),
        &history(event()),
        now(),
        &tz(),
    );
    assert!(alerts[0].title.contains("announced"));
    assert!(
        alerts[0]
            .message
            .contains("Time has passed; landing is not yet confirmed")
    );
    assert!(alerts[0].message.contains("7:00 a.m. PDT"));
}

#[test]
fn labels_banked_credits_without_claiming_current_usage_was_restored() {
    let alerts = select_reset_alerts(
        &feed(vec![with("action", "action_claimed")]),
        &history(HistoryEvent {
            kind: "banked".into(),
            evidence_class: "measured_account".into(),
            status: "recorded".into(),
            event_kind: "policy_change".into(),
            ..event()
        }),
        now(),
        &tz(),
    );
    assert!(alerts[0].message.contains("Banked credit"));
    assert!(
        alerts[0]
            .message
            .contains("current usage is unchanged until redeemed")
    );
}

#[test]
fn does_not_infer_a_reset_type_without_matching_history_evidence() {
    let alerts = select_reset_alerts(
        &feed(vec![item()]),
        &ResetHistory { items: vec![] },
        now(),
        &tz(),
    );
    assert!(alerts[0].message.contains("Reset type unspecified"));
}

#[test]
fn does_not_promote_a_banked_policy_promise_mislabeled_action_claimed_to_landed() {
    let alerts = select_reset_alerts(
        &feed(vec![FeedItem {
            summary:
                "Correction: it had not landed. The first banked reset will arrive in three hours."
                    .into(),
            ..with("action", "action_claimed")
        }]),
        &history(HistoryEvent {
            kind: "banked".into(),
            event_kind: "policy_change".into(),
            status: "recorded".into(),
            ..event()
        }),
        now(),
        &tz(),
    );
    assert_eq!(alerts[0].title, "Codex reset update");
    assert!(alerts[0].message.contains("Banked credit"));
    assert!(alerts[0].message.contains("it had not landed"));
}

#[test]
fn rejects_stale_future_snapshots_and_ignores_old_future_withdrawn_signals() {
    for generated in [now() - FEED_MAX_AGE_MS - 1, now() + 6 * 60_000] {
        let stale = AlertFeed {
            generated_at: to_iso_string(generated),
            items: vec![item()],
        };
        assert!(select_reset_alerts(&stale, &history(event()), now(), &tz()).is_empty());
    }
    let items = vec![
        FeedItem {
            withdrawn: true,
            ..item()
        },
        FeedItem {
            published_at: to_iso_string(now() - ALERT_LOOKBACK_MS - 1),
            ..item()
        },
        FeedItem {
            published_at: to_iso_string(now() + 6 * 60_000),
            ..item()
        },
    ];
    assert!(select_reset_alerts(&feed(items), &history(event()), now(), &tz()).is_empty());
}

#[test]
fn suppresses_fulfilled_superseded_and_expired_previews() {
    for status in ["fulfilled", "completed", "expired", "missed", "superseded"] {
        let entry = HistoryEvent {
            status: status.into(),
            ..event()
        };
        assert!(select_reset_alerts(&feed(vec![item()]), &history(entry), now(), &tz()).is_empty());
    }
    let fulfilled = HistoryEvent {
        fulfilled_by: Some("new-reset".into()),
        ..event()
    };
    assert!(select_reset_alerts(&feed(vec![item()]), &history(fulfilled), now(), &tz()).is_empty());
}

#[test]
fn sends_only_the_current_stage_on_enrollment_and_deduplicates_revisions() {
    let likely = with("likely", "likely_forecast");
    let revision = FeedItem {
        id: "revision-2".into(),
        ..item()
    };
    assert_eq!(
        select_reset_alerts(
            &feed(vec![likely.clone(), item(), revision]),
            &history(event()),
            now(),
            &tz()
        )
        .len(),
        1
    );
    let action = with("action", "action_claimed");
    let alerts = select_reset_alerts(
        &feed(vec![likely, item(), action]),
        &history(completed()),
        now(),
        &tz(),
    );
    assert_eq!(alerts.len(), 1);
    assert!(alerts[0].title.contains("landed"));
}

#[test]
fn retains_distinct_keys_for_schedule_type_changes_and_stages() {
    let first = select_reset_alerts(&feed(vec![item()]), &history(event()), now(), &tz()).remove(0);
    let changed = select_reset_alerts(
        &feed(vec![FeedItem {
            target_at: Some("2026-10-03T17:00:00Z".into()),
            ..item()
        }]),
        &history(event()),
        now(),
        &tz(),
    )
    .remove(0);
    let banked = select_reset_alerts(
        &feed(vec![item()]),
        &history(HistoryEvent {
            kind: "banked".into(),
            ..event()
        }),
        now(),
        &tz(),
    )
    .remove(0);
    assert_ne!(first.key, changed.key);
    assert_ne!(first.aliases, changed.aliases);
    assert_ne!(first.key, banked.key);
}

#[test]
fn bounds_notification_length_while_preserving_the_source_link() {
    let alerts = select_reset_alerts(
        &feed(vec![FeedItem {
            summary: "A".repeat(16_000),
            source_url: format!("https://example.com/{}", "x".repeat(490)),
            ..item()
        }]),
        &history(event()),
        now(),
        &tz(),
    );
    assert!(utf16_len(&alerts[0].message) <= 500);
    assert!(alerts[0].url.contains("https://example.com/"));
    assert!(!alerts[0].message.contains("https://"));
}

#[test]
fn keeps_a_landed_push_compact_even_when_the_source_summary_embeds_a_whole_reply_thread() {
    let alerts = select_reset_alerts(
        &feed(vec![FeedItem {
            target_at: None,
            source_published_at: Some("2026-10-02T15:18:48Z".into()),
            summary: format!(
                "Asked “{}”, the lead replied: “Reset all propagated. Enjoy. https://t.”",
                "Long earlier announcement. ".repeat(20)
            ),
            ..with("action", "action_claimed")
        }]),
        &history(completed()),
        now(),
        &tz(),
    );
    let alert = &alerts[0];
    assert_eq!(alert.title, "Codex reset landed");
    assert!(
        alert
            .message
            .contains("Reported complete. Check your Usage page.")
    );
    assert!(alert.message.contains("Posted Oct 2, 8:18 a.m. PDT."));
    assert!(!alert.message.contains("Asked"));
    assert!(utf16_len(&alert.message) < 220);
    assert_eq!(alert.key, "october-reset:landed:non-banked");
    assert_eq!(alert.aliases, vec!["post:123:landed:non-banked".to_owned()]);
}

#[test]
fn keeps_predictions_readable_and_never_includes_broken_tracking_urls() {
    let alerts = select_reset_alerts(
        &feed(vec![FeedItem {
            target_at: None,
            summary: format!(
                "85% chance within 24 hours. https://t. {}",
                "Supporting detail. ".repeat(40)
            ),
            ..with("likely", "likely_forecast")
        }]),
        &history(event()),
        now(),
        &tz(),
    );
    let alert = &alerts[0];
    assert!(alert.message.contains("85% chance within 24 hours."));
    assert!(
        alert
            .message
            .contains("Prediction or hint, not confirmation.")
    );
    assert!(!alert.message.contains("https://t"));
    assert!(utf16_len(&alert.message) < 400);
}

#[test]
fn fails_source_decoding_on_invalid_timestamps_unsafe_urls_and_missing_fields() {
    let item_json = json!({
        "id": "announcement:revision-1",
        "eventId": "october-reset",
        "postId": "123",
        "topic": "schedule",
        "state": "official_scheduled",
        "title": "Reset scheduled",
        "summary": "An official post names a reset time.",
        "sourceUrl": "https://x.com/thsottiaux/status/123",
        "evidenceId": "capture-123",
        "targetAt": "2026-10-02T17:00:00Z",
        "publishedAt": "2026-10-02T15:00:00Z",
        "withdrawn": false,
    });
    let generated = to_iso_string(now());
    let feed_json = |items: serde_json::Value| json!({"generatedAt": generated, "items": items});
    assert!(
        decode_alert_feed(
            json!({"generatedAt": "not a date", "items": [item_json.clone()]}),
            &tz()
        )
        .is_err()
    );
    let mut unsafe_url = item_json.clone();
    unsafe_url["sourceUrl"] = json!("javascript:alert(1)");
    assert!(decode_alert_feed(feed_json(json!([unsafe_url])), &tz()).is_err());
    assert!(decode_alert_feed(json!({"generatedAt": generated}), &tz()).is_err());
    let decoded = decode_alert_feed(feed_json(json!([item_json])), &tz()).unwrap();
    assert_eq!(decoded, feed(vec![item()]));
}
