//! Reset Beacon history decoding.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use jiff::tz::TimeZone;
use omni_core::js::to_iso_string;
use omni_personal::codex_resets::history::add_completed_history_alerts;
use omni_personal::codex_resets::source::{
    AlertFeed, FeedItem, HistoryEvent, HistorySource, ResetHistory,
};

const ANNOUNCED_AT: &str = "2026-10-02T11:00:00Z";

fn tz() -> TimeZone {
    TimeZone::get("America/Vancouver").unwrap()
}

fn now() -> i64 {
    "2026-10-02T12:00:00Z"
        .parse::<jiff::Timestamp>()
        .unwrap()
        .as_millisecond()
}

fn feed() -> AlertFeed {
    AlertFeed {
        generated_at: to_iso_string(now()),
        items: vec![],
    }
}

fn source(id: &str, url: &str) -> HistorySource {
    HistorySource {
        announcement_id: id.into(),
        url: url.into(),
    }
}

fn completed_event() -> HistoryEvent {
    HistoryEvent {
        id: "completion-1".into(),
        kind: "special_global".into(),
        scope: "all".into(),
        event_kind: "completed".into(),
        evidence_class: "reported".into(),
        status: "completed".into(),
        fulfilled_by: None,
        superseded_by: None,
        announced_at: Some(ANNOUNCED_AT.into()),
        summary: Some("The reset completed.".into()),
        sources: vec![source("post-completion", "https://resetbeacon.com/post/1")],
    }
}

fn history(items: Vec<HistoryEvent>) -> ResetHistory {
    ResetHistory { items }
}

fn scheduled_parent(completion_id: &str) -> HistoryEvent {
    HistoryEvent {
        id: format!("parent-{completion_id}"),
        kind: "special_global".into(),
        scope: "all".into(),
        event_kind: "scheduled".into(),
        evidence_class: "named_public_source".into(),
        status: "fulfilled".into(),
        fulfilled_by: Some(completion_id.into()),
        superseded_by: None,
        announced_at: None,
        summary: None,
        sources: vec![source(
            &format!("post-parent-{completion_id}"),
            "https://resetbeacon.com/parent",
        )],
    }
}

fn linked_history(event: HistoryEvent) -> ResetHistory {
    history(vec![scheduled_parent(&event.id), event])
}

fn feed_item(id: &str, event_id: &str, post_id: &str, topic: &str, state: &str) -> FeedItem {
    FeedItem {
        id: id.into(),
        event_id: event_id.into(),
        post_id: Some(post_id.into()),
        topic: topic.into(),
        state: state.into(),
        title: "Scheduled".into(),
        summary: "Earlier announcement".into(),
        source_url: "https://resetbeacon.com/parent".into(),
        evidence_id: None,
        target_at: Some(ANNOUNCED_AT.into()),
        published_at: ANNOUNCED_AT.into(),
        source_published_at: None,
        withdrawn: false,
    }
}

fn with_parent(base: AlertFeed, completion_id: &str) -> AlertFeed {
    let mut items = vec![feed_item(
        "parent-feed",
        "existing-event",
        &format!("post-parent-{completion_id}"),
        "schedule",
        "official_scheduled",
    )];
    items.extend(base.items);
    AlertFeed {
        generated_at: base.generated_at,
        items,
    }
}

fn withdrawn_item(post_id: &str, url: &str) -> FeedItem {
    FeedItem {
        title: "Old claim".into(),
        summary: "Withdrawn".into(),
        source_url: url.into(),
        target_at: None,
        withdrawn: true,
        ..feed_item("withdrawn", "event-1", post_id, "action", "action_claimed")
    }
}

#[test]
fn adds_a_recent_completed_global_reset_with_its_source_announcement() {
    let result = add_completed_history_alerts(
        &with_parent(feed(), "completion-1"),
        &linked_history(completed_event()),
        now(),
        &tz(),
    );
    assert_eq!(result.generated_at, feed().generated_at);
    assert_eq!(
        result.items[1..],
        [FeedItem {
            id: "history:completion-1".into(),
            event_id: "existing-event".into(),
            post_id: Some("post-completion".into()),
            topic: "action".into(),
            state: "action_claimed".into(),
            title: "Codex allowance reset completed".into(),
            summary: "The reset completed.".into(),
            source_url: "https://resetbeacon.com/post/1".into(),
            evidence_id: None,
            target_at: None,
            published_at: ANNOUNCED_AT.into(),
            source_published_at: Some(ANNOUNCED_AT.into()),
            withdrawn: false,
        }]
    );
}

#[test]
fn omits_stale_too_far_future_and_non_completed_history() {
    let base = completed_event;
    let cases = vec![
        HistoryEvent {
            id: "stale".into(),
            announced_at: Some("2026-09-30T11:59:59Z".into()),
            ..base()
        },
        HistoryEvent {
            id: "future".into(),
            announced_at: Some("2026-10-02T12:05:01Z".into()),
            ..base()
        },
        HistoryEvent {
            id: "preview".into(),
            event_kind: "policy_change".into(),
            status: "announced".into(),
            ..base()
        },
        HistoryEvent {
            id: "banked".into(),
            kind: "banked".into(),
            ..base()
        },
        HistoryEvent {
            id: "expired".into(),
            status: "expired".into(),
            ..base()
        },
        HistoryEvent {
            id: "superseded".into(),
            superseded_by: Some("replacement".into()),
            ..base()
        },
        HistoryEvent {
            id: "unlinked".into(),
            scope: "pro".into(),
            ..base()
        },
    ];
    for event in cases {
        let input = with_parent(feed(), &event.id);
        let result = add_completed_history_alerts(&input, &linked_history(event), now(), &tz());
        assert_eq!(result.items, input.items);
    }
}

#[test]
fn requires_an_explicitly_fulfilled_scheduled_parent() {
    let event = completed_event();
    let input = with_parent(feed(), "completion-1");
    assert_eq!(
        add_completed_history_alerts(&input, &history(vec![event.clone()]), now(), &tz()).items,
        input.items
    );
    let active = HistoryEvent {
        status: "active".into(),
        ..scheduled_parent(&event.id)
    };
    assert_eq!(
        add_completed_history_alerts(&input, &history(vec![active, event.clone()]), now(), &tz())
            .items,
        input.items
    );
    let banked = HistoryEvent {
        kind: "banked".into(),
        ..scheduled_parent(&event.id)
    };
    assert_eq!(
        add_completed_history_alerts(&input, &history(vec![banked, event]), now(), &tz()).items,
        input.items
    );
}

#[test]
fn does_not_create_a_new_delivery_identity_if_the_parent_feed_item_disappeared() {
    assert!(
        add_completed_history_alerts(&feed(), &linked_history(completed_event()), now(), &tz())
            .items
            .is_empty()
    );
}

#[test]
fn does_not_bypass_a_withdrawn_feed_post() {
    let withdrawn = AlertFeed {
        items: vec![withdrawn_item(
            "post-completion",
            "https://resetbeacon.com/post/1",
        )],
        ..feed()
    };
    assert_eq!(
        add_completed_history_alerts(
            &with_parent(withdrawn, "completion-1"),
            &linked_history(completed_event()),
            now(),
            &tz()
        )
        .items
        .len(),
        2
    );
}

#[test]
fn adds_completion_beside_an_earlier_likely_or_scheduled_post() {
    let preview = AlertFeed {
        items: vec![FeedItem {
            title: "Reset scheduled".into(),
            summary: "A reset was announced.".into(),
            source_url: "https://resetbeacon.com/post/1".into(),
            ..feed_item(
                "preview",
                "event-1",
                "post-completion",
                "schedule",
                "official_scheduled",
            )
        }],
        ..feed()
    };
    let result = add_completed_history_alerts(
        &with_parent(preview, "completion-1"),
        &linked_history(completed_event()),
        now(),
        &tz(),
    );
    assert_eq!(result.items.len(), 3);
    assert_eq!(result.items[2].id, "history:completion-1");
    assert_eq!(result.items[2].post_id.as_deref(), Some("post-completion"));
}

#[test]
fn checks_all_announcement_sources_for_a_withdrawn_or_completed_feed_post() {
    let multi = HistoryEvent {
        sources: vec![
            source("post-new", "https://resetbeacon.com/post/new"),
            source("post-withdrawn", "https://resetbeacon.com/post/old"),
        ],
        ..completed_event()
    };
    let withdrawn = AlertFeed {
        items: vec![FeedItem {
            event_id: "event-old".into(),
            ..withdrawn_item("post-withdrawn", "https://resetbeacon.com/post/old")
        }],
        ..feed()
    };
    assert_eq!(
        add_completed_history_alerts(
            &with_parent(withdrawn, "completion-1"),
            &linked_history(multi),
            now(),
            &tz()
        )
        .items
        .len(),
        2
    );
}

#[test]
fn reuses_the_parent_feed_event_id_through_fulfilled_by() {
    let parent = HistoryEvent {
        id: "parent".into(),
        event_kind: "scheduled".into(),
        status: "fulfilled".into(),
        fulfilled_by: Some("completion-1".into()),
        announced_at: None,
        sources: vec![source("post-parent", "https://resetbeacon.com/post/old")],
        ..completed_event()
    };
    let parent_feed = AlertFeed {
        items: vec![FeedItem {
            source_url: "https://resetbeacon.com/post/old".into(),
            ..feed_item(
                "parent-feed-item",
                "existing-event",
                "post-parent",
                "schedule",
                "official_scheduled",
            )
        }],
        ..feed()
    };
    let result = add_completed_history_alerts(
        &parent_feed,
        &history(vec![parent, completed_event()]),
        now(),
        &tz(),
    );
    let last = result.items.last().unwrap();
    assert_eq!(last.event_id, "existing-event");
    assert_eq!(last.post_id.as_deref(), Some("post-completion"));
}
