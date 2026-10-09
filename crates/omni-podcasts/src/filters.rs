//! Pure hard filters applied before any model call (`filters.ts`).

use std::collections::HashSet;

use crate::persistence::PodcastExclusions;
use crate::titles::normalize_title;
use crate::types::{CanonicalShowId, EpisodeCandidate};

const DAY_MS: i64 = 24 * 60 * 60 * 1000;
/// Only episodes released within this window are recommendable.
pub const RECENT_EPISODE_WINDOW_MS: i64 = 7 * DAY_MS;

pub struct PodcastFilterContext<'a> {
    pub now: i64,
    pub subscribed_show_ids: &'a HashSet<CanonicalShowId>,
    /// Normalized titles of subscribed shows (fallback when ids are missing).
    pub subscribed_show_titles: &'a HashSet<String>,
    pub exclusions: &'a PodcastExclusions,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DroppedEpisode {
    pub candidate: EpisodeCandidate,
    pub reason: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PodcastFilterResult {
    pub kept: Vec<EpisodeCandidate>,
    pub dropped: Vec<DroppedEpisode>,
}

pub fn filter_eligible_episodes(
    candidates: Vec<EpisodeCandidate>,
    context: &PodcastFilterContext<'_>,
) -> PodcastFilterResult {
    let mut result = PodcastFilterResult::default();
    for candidate in candidates {
        match disqualify(&candidate, context) {
            Some(reason) => result.dropped.push(DroppedEpisode { candidate, reason }),
            None => result.kept.push(candidate),
        }
    }
    result
}

fn disqualify(candidate: &EpisodeCandidate, context: &PodcastFilterContext<'_>) -> Option<String> {
    let age = context.now - candidate.published_at;
    if age > RECENT_EPISODE_WINDOW_MS {
        let days = age.div_euclid(DAY_MS);
        return Some(format!("released {days}d ago (outside recency window)"));
    }
    if candidate.published_at > context.now + DAY_MS {
        return Some("release date in the future (feed metadata suspect)".to_owned());
    }
    if context
        .exclusions
        .episode_ids
        .contains(&candidate.episode_id)
    {
        return Some("episode already recommended".to_owned());
    }
    if context.exclusions.show_ids.contains(&candidate.show_id) {
        return Some("show on cooldown or excluded by feedback".to_owned());
    }
    if context.subscribed_show_ids.contains(&candidate.show_id) {
        return Some("already subscribed".to_owned());
    }
    if context
        .subscribed_show_titles
        .contains(&normalize_title(&candidate.show_title))
    {
        return Some("already subscribed (title match)".to_owned());
    }
    None
}
