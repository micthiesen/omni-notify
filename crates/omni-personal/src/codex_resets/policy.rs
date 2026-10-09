//! Codex alert selection (`src/codex-resets/policy.ts`). The feed supplies
//! classification; history supplies type and reconciles old hints. An elapsed
//! announcement deadline never constitutes evidence of a landed reset.

use std::collections::HashMap;

use indexmap::IndexMap;
use jiff::tz::TimeZone;
use omni_http::Url;

use super::source::{AlertFeed, FeedItem, HistoryEvent, ResetHistory};
use crate::js::parse_date;
use crate::reset_alerts::ResetAlert;
use crate::reset_alerts::presentation::{
    ALERT_LOOKBACK_MS, CLOCK_SKEW_MS, compact_summary, pacific_time,
};

/// Feed snapshots older than this fail the task visibly.
pub const FEED_MAX_AGE_MS: i64 = 45 * 60_000;

/// The feed was generated within the last 45 minutes (5 minutes of skew allowed).
pub fn is_fresh_feed(feed: &AlertFeed, now: i64, tz: &TimeZone) -> bool {
    let Some(generated) = parse_date(&feed.generated_at, tz) else {
        return false;
    };
    let age = now - generated;
    (-CLOCK_SKEW_MS..=FEED_MAX_AGE_MS).contains(&age)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Likely,
    Scheduled,
    Rollout,
    Landed,
    Update,
}

impl Stage {
    fn as_str(self) -> &'static str {
        match self {
            Stage::Likely => "likely",
            Stage::Scheduled => "scheduled",
            Stage::Rollout => "rollout",
            Stage::Landed => "landed",
            Stage::Update => "update",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Stage::Likely => "looks likely",
            Stage::Scheduled => "announced",
            Stage::Rollout => "rolling out",
            Stage::Landed => "landed",
            Stage::Update => "update",
        }
    }
}

fn stage_of(item: &FeedItem, event: Option<&HistoryEvent>) -> Option<Stage> {
    match (item.topic.as_str(), item.state.as_str()) {
        ("likely", "likely_forecast" | "likely_official_intent") => Some(Stage::Likely),
        ("schedule", "official_scheduled") => Some(Stage::Scheduled),
        ("rollout", "rollout_observed") => Some(Stage::Rollout),
        ("action", "action_claimed") => {
            // Banked promises and receipts both appear as action_claimed/policy_change.
            // Require completion or measured receipt evidence before claiming a landing.
            let completed =
                event.is_some_and(|e| e.event_kind == "completed" && e.status == "completed");
            let measured_banked =
                event.is_some_and(|e| e.kind == "banked" && e.evidence_class == "measured_account");
            Some(if completed || measured_banked {
                Stage::Landed
            } else {
                Stage::Update
            })
        }
        _ => None,
    }
}

fn source_label(source_url: &str) -> String {
    let Ok(source) = Url::parse(source_url) else {
        return "Reset Beacon".to_owned();
    };
    let host = source.host_str().unwrap_or_default();
    if ["x.com", "twitter.com", "www.x.com", "www.twitter.com"].contains(&host) {
        let handle = source.path().split('/').nth(1).unwrap_or_default();
        let valid = (1..=15).contains(&handle.len())
            && handle
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_');
        if valid {
            return format!("@{handle} via Reset Beacon");
        }
    }
    if host == "resetbeacon.com" {
        "Reset Beacon".to_owned()
    } else {
        let short: String = host.chars().take(80).collect();
        format!("{short} via Reset Beacon")
    }
}

fn scope_label(scope: &str) -> &'static str {
    match scope {
        "all" => "all users",
        "plus_pro" => "Plus / Pro",
        "pro" => "Pro",
        "model" => "model-specific",
        _ => "scope unspecified",
    }
}

fn truthy(value: Option<&str>) -> Option<&str> {
    value.filter(|s| !s.is_empty())
}

/// Selects the current, deduplicated Codex reset alerts.
pub fn select_reset_alerts(
    feed: &AlertFeed,
    history: &ResetHistory,
    now: i64,
    tz: &TimeZone,
) -> Vec<ResetAlert> {
    if !is_fresh_feed(feed, now, tz) {
        return Vec::new();
    }
    let mut by_post: HashMap<&str, &HistoryEvent> = HashMap::new();
    for event in &history.items {
        for source in &event.sources {
            by_post.insert(source.announcement_id.as_str(), event);
        }
    }
    let live_items: Vec<&FeedItem> = feed.items.iter().filter(|item| !item.withdrawn).collect();
    let event_for = |item: &FeedItem| -> Option<&HistoryEvent> {
        truthy(item.post_id.as_deref()).and_then(|post| by_post.get(post).copied())
    };
    let date = |value: &str| parse_date(value, tz);

    let mut candidates: Vec<ResetAlert> = Vec::new();
    for item in &live_items {
        let event = event_for(item);
        let Some(stage) = stage_of(item, event) else {
            continue;
        };
        let Some(occurred_at) = date(&item.published_at) else {
            continue;
        };
        if now - occurred_at > ALERT_LOOKBACK_MS || occurred_at > now + CLOCK_SKEW_MS {
            continue;
        }
        let event_status = event.map_or("", |e| e.status.as_str());
        if event.is_some_and(|e| truthy(e.superseded_by.as_deref()).is_some())
            || ["superseded", "expired", "missed"].contains(&event_status)
        {
            continue;
        }
        let target_ms = truthy(item.target_at.as_deref()).and_then(date);
        if matches!(stage, Stage::Likely | Stage::Scheduled) {
            if event.is_some_and(|e| truthy(e.fulfilled_by.as_deref()).is_some())
                || ["fulfilled", "completed"].contains(&event_status)
            {
                continue;
            }
            let later_stage = live_items.iter().any(|other| {
                other.event_id == item.event_id
                    && matches!(
                        stage_of(other, event_for(other)),
                        Some(Stage::Landed | Stage::Rollout | Stage::Update)
                    )
            });
            if later_stage {
                continue;
            }
            if stage == Stage::Likely
                && live_items.iter().any(|other| {
                    other.event_id == item.event_id
                        && stage_of(other, None) == Some(Stage::Scheduled)
                })
            {
                continue;
            }
            if stage == Stage::Likely
                && truthy(item.target_at.as_deref()).is_some()
                && target_ms.is_some_and(|t| t <= now)
            {
                continue;
            }
        }
        let kind = event.map(|e| e.kind.as_str());
        let reset_type = match kind {
            Some("banked") => "banked",
            Some("special_global") => "non-banked",
            _ => "unspecified",
        };
        let scope = event.map_or("scope unspecified", |e| scope_label(&e.scope));
        let type_line = format!(
            "{}; {scope}.",
            match reset_type {
                "unspecified" => "Reset type unspecified",
                "banked" => "Banked credit",
                _ => "Non-banked reset",
            }
        );
        let target_at = truthy(item.target_at.as_deref());
        let timing = match (target_at, stage) {
            (Some(target), Stage::Scheduled) => {
                let passed = target_ms.is_some_and(|t| t <= now);
                format!(
                    "Expected {}.{}",
                    target_ms
                        .map(pacific_time)
                        .unwrap_or_else(|| target.to_owned()),
                    if passed {
                        " Time has passed; landing is not yet confirmed."
                    } else {
                        ""
                    }
                )
            }
            _ => String::new(),
        };
        let news = match stage {
            Stage::Landed => "Reported complete. Check your Usage page.".to_owned(),
            Stage::Rollout => {
                "Observed on the tracker's account; your account may update later.".to_owned()
            }
            Stage::Scheduled if timing.is_empty() => {
                "A reset has been announced; timing is unspecified.".to_owned()
            }
            Stage::Scheduled => timing.clone(),
            Stage::Likely | Stage::Update => compact_summary(&item.summary),
        };
        // `!item.targetAt || Date.parse(item.targetAt) > now` (NaN compares false).
        let not_yet_due = target_at.is_none() || target_ms.is_some_and(|t| t > now);
        let guidance = if reset_type == "banked" {
            "Saved credit; current usage is unchanged until redeemed."
        } else {
            match stage {
                Stage::Likely => "Prediction or hint, not confirmation.",
                Stage::Update => "Completion is not established.",
                Stage::Scheduled if not_yet_due => "Use remaining allowance beforehand if useful.",
                _ => "",
            }
        };
        let posted = truthy(item.source_published_at.as_deref())
            .and_then(date)
            .map(|ms| format!("Posted {}.", pacific_time(ms)))
            .unwrap_or_default();
        let schedule_suffix = if stage == Stage::Scheduled {
            format!(":{}", item.target_at.as_deref().unwrap_or("unspecified"))
        } else {
            String::new()
        };
        let identity = format!("{}:{reset_type}{schedule_suffix}", stage.as_str());
        let source = format!("Source: {}", source_label(&item.source_url));
        let message = [
            type_line.as_str(),
            news.as_str(),
            guidance,
            posted.as_str(),
            source.as_str(),
        ]
        .into_iter()
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
        candidates.push(ResetAlert {
            // Ignore cosmetic feed revisions, but retain stage, type and schedule changes.
            key: format!("{}:{identity}", item.event_id),
            aliases: truthy(item.post_id.as_deref())
                .map(|post| vec![format!("post:{post}:{identity}")])
                .unwrap_or_default(),
            title: format!("Codex reset {}", stage.label()),
            message,
            url: item.source_url.clone(),
            occurred_at,
        });
    }
    // A newly enrolled monitor sends current news, not every revision of that news.
    candidates.sort_by_key(|alert| alert.occurred_at);
    let mut unique: IndexMap<String, ResetAlert> = IndexMap::new();
    for alert in candidates {
        unique.insert(alert.key.clone(), alert);
    }
    unique.into_values().collect()
}
