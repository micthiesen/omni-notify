//! Completed-history fallback: report a
//! completed reset while the alert feed is delayed. Feed entries remain
//! authoritative when they already cover a post.

use std::collections::HashSet;

use jiff::tz::TimeZone;

use super::source::{AlertFeed, FeedItem, ResetHistory};
use crate::js::parse_date;
use crate::reset_alerts::presentation::{ALERT_LOOKBACK_MS, CLOCK_SKEW_MS};

/// Appends a synthetic `action_claimed` item for each recent, explicitly
/// completed global reset whose fulfilled scheduled parent is still in the feed.
pub fn add_completed_history_alerts(
    feed: &AlertFeed,
    history: &ResetHistory,
    now: i64,
    tz: &TimeZone,
) -> AlertFeed {
    let mut items = feed.items.clone();
    for event in &history.items {
        if event.event_kind != "completed"
            || event.status != "completed"
            || event.kind != "special_global"
            || event.scope != "all"
            || event
                .superseded_by
                .as_deref()
                .is_some_and(|s| !s.is_empty())
        {
            continue;
        }
        let Some(announced_raw) = event.announced_at.as_deref().filter(|s| !s.is_empty()) else {
            continue;
        };
        let Some(announced_at) = parse_date(announced_raw, tz) else {
            continue;
        };
        if announced_at < now - ALERT_LOOKBACK_MS || announced_at > now + CLOCK_SKEW_MS {
            continue;
        }
        let announcement_ids: HashSet<&str> = event
            .sources
            .iter()
            .map(|source| source.announcement_id.as_str())
            .collect();
        let covered_by_feed = items.iter().any(|item| {
            item.post_id
                .as_deref()
                .is_some_and(|post| announcement_ids.contains(post))
                && (item.withdrawn || item.topic == "action" || item.topic == "rollout")
        });
        let Some(source) = event.sources.first() else {
            continue;
        };
        if covered_by_feed {
            continue;
        }
        let parent = history
            .items
            .iter()
            .find(|candidate| candidate.fulfilled_by.as_deref() == Some(event.id.as_str()));
        let Some(parent) = parent else {
            continue;
        };
        if parent.kind != event.kind
            || !["completed", "fulfilled"].contains(&parent.status.as_str())
            || !["scheduled", "intent"].contains(&parent.event_kind.as_str())
            || parent.scope != "all"
        {
            continue;
        }
        let parent_feed_item = items.iter().find(|item| {
            parent
                .sources
                .iter()
                .any(|ps| item.post_id.as_deref() == Some(ps.announcement_id.as_str()))
        });
        // The old delivery ledger only knows feed event IDs. If the parent has
        // disappeared, a synthetic ID could replay a pre-upgrade notification.
        let Some(parent_feed_item) = parent_feed_item else {
            continue;
        };
        let synthetic = FeedItem {
            id: format!("history:{}", event.id),
            event_id: parent_feed_item.event_id.clone(),
            post_id: Some(source.announcement_id.clone()),
            topic: "action".to_owned(),
            state: "action_claimed".to_owned(),
            title: "Codex allowance reset completed".to_owned(),
            summary: event.summary.clone().unwrap_or_else(|| {
                "Reset Beacon reports that the Codex allowance reset completed.".to_owned()
            }),
            source_url: source.url.clone(),
            evidence_id: None,
            target_at: None,
            published_at: announced_raw.to_owned(),
            source_published_at: Some(announced_raw.to_owned()),
            withdrawn: false,
        };
        items.push(synthetic);
    }
    AlertFeed {
        generated_at: feed.generated_at.clone(),
        items,
    }
}
