//! The aggregate transition decision for one tick.
//!
//! Notifications happen only on aggregate offline-to-live and live-to-offline
//! edges. The first live binding is the sticky primary for the session (a
//! switch is silent), except that YouTube always supersedes a live Kick
//! primary. Viewer counts sum the currently live bindings.

use omni_store::cbor::Extra;

use crate::platform::{FetchedLive, FetchedStatus, Platform, PlatformBinding};
use crate::status::{LiveSource, LiveStatus, OfflineStatus, StreamerStatus};

/// One binding's observation this tick.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BindingFetchResult {
    pub binding: PlatformBinding,
    pub status: FetchedStatus,
}

/// What the tick means for the streamer.
#[derive(Clone, Debug, PartialEq)]
pub enum TickDecision {
    AllUnknown {
        errors: Vec<String>,
    },
    NoChange,
    WentLive {
        next: LiveStatus,
        summed_viewer_count: i64,
    },
    WentOffline {
        previous_live: LiveStatus,
        next: OfflineStatus,
    },
    StillLive {
        next: LiveStatus,
        summed_viewer_count: i64,
        /// Primary unchanged and its title changed: the only title-change case.
        title_changed: bool,
        primary_switched: bool,
    },
}

/// Pure transition decision.
pub fn decide_transition(
    streamer_id: &str,
    previous: &StreamerStatus,
    results: &[BindingFetchResult],
    now_ms: i64,
) -> TickDecision {
    let unknown_errors: Vec<String> = results
        .iter()
        .filter_map(|r| match &r.status {
            FetchedStatus::Unknown { error } => Some(error.clone()),
            _ => None,
        })
        .collect();
    let lives: Vec<(&PlatformBinding, &FetchedLive)> = results
        .iter()
        .filter_map(|r| match &r.status {
            FetchedStatus::Live(live) => Some((&r.binding, live)),
            _ => None,
        })
        .collect();

    if !results.is_empty() && unknown_errors.len() == results.len() {
        return TickDecision::AllUnknown {
            errors: unknown_errors,
        };
    }

    if lives.is_empty() {
        // Some unknown (might still be live there) or fully offline: no
        // transition unless previously live and every binding confirmed offline.
        let StreamerStatus::Live(previous_live) = previous else {
            return TickDecision::NoChange;
        };
        if !unknown_errors.is_empty() {
            return TickDecision::NoChange;
        }
        let next = OfflineStatus {
            streamer_id: streamer_id.to_owned(),
            last_ended_at: Some(now_ms),
            last_started_at: Some(previous_live.started_at),
            last_max_viewer_count: Some(previous_live.max_viewer_count),
            extra: Extra::new(),
        };
        return TickDecision::WentOffline {
            previous_live: previous_live.clone(),
            next,
        };
    }

    let summed_viewer_count: i64 = lives.iter().map(|(_, l)| l.viewer_count.unwrap_or(0)).sum();
    let sources: Vec<LiveSource> = lives
        .iter()
        .map(|(binding, live)| LiveSource {
            platform: binding.platform,
            username: binding.username.clone(),
            title: live.title.clone(),
            viewer_count: live.viewer_count,
            category: live.category.clone(),
        })
        .collect();
    // Stable: the first live binding of the highest-priority platform.
    let priority_primary = lives
        .iter()
        .min_by_key(|(binding, _)| binding.platform.priority())
        .map(|(binding, _)| (*binding).clone());
    let Some(priority_primary) = priority_primary else {
        return TickDecision::NoChange;
    };

    let (primary, primary_switched) = match previous {
        StreamerStatus::Offline(_) => (priority_primary, false),
        StreamerStatus::Live(previous_live) => {
            let previous_still_live = lives
                .iter()
                .any(|(binding, _)| binding.same_account(&previous_live.primary));
            let youtube_supersedes_kick = previous_live.primary.platform == Platform::Kick
                && priority_primary.platform == Platform::YouTube;
            if previous_still_live && !youtube_supersedes_kick {
                (previous_live.primary.clone(), false)
            } else {
                (priority_primary, true)
            }
        }
    };

    let Some((_, primary_live)) = lives
        .iter()
        .find(|(binding, _)| binding.same_account(&primary))
    else {
        // The primary is always one of the live bindings.
        return TickDecision::NoChange;
    };
    let primary_title = primary_live.title.clone();

    match previous {
        StreamerStatus::Offline(_) => {
            let reported = primary_live
                .started_at
                .as_deref()
                .and_then(parse_js_date)
                .filter(|start| *start <= now_ms);
            let next = LiveStatus {
                streamer_id: streamer_id.to_owned(),
                primary,
                primary_title,
                started_at: reported.unwrap_or(now_ms),
                max_viewer_count: summed_viewer_count,
                viewer_count: Some(summed_viewer_count),
                sources: Some(sources),
                category: primary_live.category.clone(),
                extra: Extra::new(),
            };
            TickDecision::WentLive {
                next,
                summed_viewer_count,
            }
        }
        StreamerStatus::Live(previous_live) => {
            // Category is display-only: a transient fetch without one keeps the
            // last value, but a primary switch takes the new primary's.
            let category = primary_live.category.clone().or_else(|| {
                if primary_switched {
                    None
                } else {
                    previous_live.category.clone()
                }
            });
            let title_changed = !primary_switched && primary_title != previous_live.primary_title;
            let next = LiveStatus {
                streamer_id: streamer_id.to_owned(),
                primary,
                primary_title,
                started_at: previous_live.started_at,
                max_viewer_count: previous_live.max_viewer_count.max(summed_viewer_count),
                viewer_count: Some(summed_viewer_count),
                sources: Some(sources),
                category,
                extra: Extra::new(),
            };
            TickDecision::StillLive {
                next,
                summed_viewer_count,
                title_changed,
                primary_switched,
            }
        }
    }
}

/// `new Date(string).getTime()` for the ISO-8601 forms platforms report;
/// `None` for an invalid date.
pub fn parse_js_date(value: &str) -> Option<i64> {
    if let Ok(timestamp) = value.parse::<jiff::Timestamp>() {
        return Some(timestamp.as_millisecond());
    }
    // Date-only and offset-less forms: JS reads date-only as UTC and a
    // local date-time as local time; platforms never send the latter.
    value
        .parse::<jiff::civil::Date>()
        .ok()
        .and_then(|date| date.to_zoned(jiff::tz::TimeZone::UTC).ok())
        .map(|zoned| zoned.timestamp().as_millisecond())
}
