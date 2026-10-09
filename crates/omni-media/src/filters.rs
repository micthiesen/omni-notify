//! Deterministic hard filters applied before any model sees a candidate.

use std::collections::HashSet;

use crate::candidates::PooledCandidate;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FilterContext {
    /// Everything ever watched (partial watches are not re-pitched).
    pub watched_ids: HashSet<String>,
    pub in_progress_ids: HashSet<String>,
    pub watchlist_ids: HashSet<String>,
    /// Recommended within cooldown, or terminally watched/abandoned.
    pub excluded_recommendation_ids: HashSet<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dropped {
    pub canonical_id: String,
    pub title: String,
    pub reason: &'static str,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FilterOutcome {
    pub kept: Vec<PooledCandidate>,
    pub dropped: Vec<Dropped>,
}

pub fn filter_eligible(pool: &[PooledCandidate], context: &FilterContext) -> FilterOutcome {
    let mut outcome = FilterOutcome::default();
    for candidate in pool {
        match drop_reason(&candidate.canonical_id, context) {
            Some(reason) => outcome.dropped.push(Dropped {
                canonical_id: candidate.canonical_id.clone(),
                title: candidate.title.clone(),
                reason,
            }),
            None => outcome.kept.push(candidate.clone()),
        }
    }
    outcome
}

fn drop_reason(canonical_id: &str, context: &FilterContext) -> Option<&'static str> {
    if context.watched_ids.contains(canonical_id) {
        Some("already watched")
    } else if context.in_progress_ids.contains(canonical_id) {
        Some("currently in progress")
    } else if context.watchlist_ids.contains(canonical_id) {
        Some("already on watchlist")
    } else if context.excluded_recommendation_ids.contains(canonical_id) {
        Some("recently recommended or terminal outcome")
    } else {
        None
    }
}
