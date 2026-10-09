//! One ordering primitive for every server-produced live list.

use std::cmp::Ordering;

use omni_api::streamers::StreamerTier;

/// The ranking inputs of a live entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LiveRank {
    pub tier: StreamerTier,
    pub viewer_count: Option<i64>,
    pub max_viewer_count: i64,
    /// Rank-only override; the displayed count stays `viewer_count`.
    pub ordering_viewer_count: Option<i64>,
}

impl LiveRank {
    fn rank(&self) -> i64 {
        self.ordering_viewer_count
            .or(self.viewer_count)
            .unwrap_or(self.max_viewer_count)
    }
}

/// Primary before background, then hottest first.
pub fn compare_live_display_order(a: &LiveRank, b: &LiveRank) -> Ordering {
    if a.tier != b.tier {
        return if a.tier == StreamerTier::Primary {
            Ordering::Less
        } else {
            Ordering::Greater
        };
    }
    b.rank().cmp(&a.rank())
}

/// Stable sort: ties keep channels.json order.
pub fn sort_live_display<T>(items: &mut [T], rank: impl Fn(&T) -> LiveRank) {
    items.sort_by(|a, b| compare_live_display_order(&rank(a), &rank(b)));
}

/// Most recently ended first; never-ended last (stable).
pub fn sort_offline_display<T>(items: &mut [T], last_ended_at: impl Fn(&T) -> Option<i64>) {
    items.sort_by_key(|item| std::cmp::Reverse(last_ended_at(item).unwrap_or(0)));
}
