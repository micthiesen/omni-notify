//! Candidate eligibility filters.
#![allow(clippy::expect_used)]

use std::collections::HashSet;

use omni_media::candidates::PooledCandidate;
use omni_media::filters::{FilterContext, filter_eligible};
use omni_media::types::{CandidateSource, MediaType, make_canonical_id};

fn make_candidate(tmdb_id: i64) -> PooledCandidate {
    PooledCandidate {
        canonical_id: make_canonical_id(MediaType::Movie, tmdb_id),
        tmdb_id,
        media_type: MediaType::Movie,
        title: format!("Movie {tmdb_id}"),
        vote_average: 7.0,
        vote_count: 1000.0,
        popularity: 10.0,
        source: CandidateSource::Trending,
        ..PooledCandidate::default()
    }
}

fn set(ids: &[&str]) -> HashSet<String> {
    ids.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn drops_watched_in_progress_watchlisted_and_excluded_candidates() {
    let pool: Vec<PooledCandidate> = (1..=5).map(make_candidate).collect();
    let outcome = filter_eligible(
        &pool,
        &FilterContext {
            watched_ids: set(&["tmdb:movie:1"]),
            in_progress_ids: set(&["tmdb:movie:2"]),
            watchlist_ids: set(&["tmdb:movie:3"]),
            excluded_recommendation_ids: set(&["tmdb:movie:4"]),
        },
    );
    assert_eq!(
        outcome.kept.iter().map(|c| c.tmdb_id).collect::<Vec<_>>(),
        vec![5]
    );
    assert_eq!(outcome.dropped.len(), 4);
    assert_eq!(
        outcome.dropped.iter().map(|d| d.reason).collect::<Vec<_>>(),
        vec![
            "already watched",
            "currently in progress",
            "already on watchlist",
            "recently recommended or terminal outcome",
        ]
    );
}

#[test]
fn keeps_everything_when_context_sets_are_empty() {
    let pool: Vec<PooledCandidate> = (1..=2).map(make_candidate).collect();
    let outcome = filter_eligible(&pool, &FilterContext::default());
    assert_eq!(outcome.kept.len(), 2);
    assert!(outcome.dropped.is_empty());
}
