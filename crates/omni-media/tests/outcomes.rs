//! Recommendation outcomes.
#![allow(clippy::expect_used)]

use std::collections::HashMap;

use omni_api::media::{RecommendationStatus, WatchlistResult};
use omni_media::outcomes::{
    ABANDONED_INACTIVITY_MS, IGNORE_WINDOW_MS, OutcomeInputs, ProgressState, WatchedState,
    decide_outcomes,
};
use omni_media::persistence::RecommendationData;
use omni_media::types::MediaType;

const NOW: i64 = 1_750_000_000_000;
const DAY: i64 = 24 * 60 * 60 * 1000;

fn make_rec(update: impl FnOnce(&mut RecommendationData)) -> RecommendationData {
    let mut rec = RecommendationData {
        recommendation_id: "rec-123".to_owned(),
        canonical_id: "tmdb:movie:123".to_owned(),
        tmdb_id: 123,
        media_type: MediaType::Movie,
        title: "Test Movie".to_owned(),
        status: RecommendationStatus::Notified,
        run_date: "2026-07-01".to_owned(),
        recommended_at: NOW - 5 * DAY,
        notified_at: Some(NOW - 5 * DAY),
        ..RecommendationData::default()
    };
    update(&mut rec);
    rec
}

fn inputs(update: impl FnOnce(&mut OutcomeInputs)) -> OutcomeInputs {
    let mut inputs = OutcomeInputs {
        watched: HashMap::new(),
        in_progress: HashMap::new(),
        in_progress_available: true,
        now: NOW,
    };
    update(&mut inputs);
    inputs
}

fn watched(completion: Option<f64>, last_viewed_at: Option<i64>) -> WatchedState {
    WatchedState {
        completion,
        view_count: 1,
        last_viewed_at,
    }
}

fn movie_history(state: WatchedState) -> HashMap<String, WatchedState> {
    HashMap::from([("tmdb:movie:123".to_owned(), state)])
}

fn movie_progress(state: ProgressState) -> HashMap<String, ProgressState> {
    HashMap::from([("tmdb:movie:123".to_owned(), state)])
}

#[test]
fn labels_watched_when_completion_is_at_or_above_the_threshold() {
    let changes = decide_outcomes(
        &[make_rec(|_| {})],
        &inputs(|i| i.watched = movie_history(watched(Some(0.92), None))),
    );
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].status, RecommendationStatus::Watched);
    assert_eq!(changes[0].reason, "completion=0.92");
}

#[test]
fn labels_watched_on_a_view_when_the_backend_reports_no_completion() {
    let changes = decide_outcomes(
        &[make_rec(|_| {})],
        &inputs(|i| i.watched = movie_history(watched(None, None))),
    );
    assert_eq!(changes[0].status, RecommendationStatus::Watched);
    assert_eq!(changes[0].reason, "viewCount=1");
}

#[test]
fn does_not_treat_one_tv_episode_view_as_completing_a_series() {
    let changes = decide_outcomes(
        &[make_rec(|r| {
            r.media_type = MediaType::Tv;
            r.canonical_id = "tmdb:tv:123".to_owned();
        })],
        &inputs(|i| {
            i.watched = HashMap::from([("tmdb:tv:123".to_owned(), watched(None, None))]);
        }),
    );
    assert!(changes.is_empty());
}

#[test]
fn does_not_label_watched_below_the_completion_threshold() {
    let changes = decide_outcomes(
        &[make_rec(|_| {})],
        &inputs(|i| {
            i.watched = movie_history(watched(Some(0.3), None));
            i.in_progress = movie_progress(ProgressState {
                progress: 0.3,
                last_viewed_at: None,
            });
        }),
    );
    assert!(changes.is_empty());
}

#[test]
fn labels_abandoned_after_a_partial_watch_has_been_inactive_for_two_weeks() {
    let changes = decide_outcomes(
        &[make_rec(|r| r.notified_at = Some(NOW - 20 * DAY))],
        &inputs(|i| {
            i.watched = movie_history(watched(Some(0.25), Some(NOW - ABANDONED_INACTIVITY_MS - 1)));
        }),
    );
    assert_eq!(changes[0].status, RecommendationStatus::Abandoned);
    assert_eq!(changes[0].reason, "stalled at 25%");
}

#[test]
fn does_not_immediately_abandon_a_recent_partial_watch() {
    let changes = decide_outcomes(
        &[make_rec(|_| {})],
        &inputs(|i| i.watched = movie_history(watched(Some(0.25), Some(NOW - 1000)))),
    );
    assert!(changes.is_empty());
}

#[test]
fn does_not_credit_watch_history_from_before_the_recommendation() {
    let delivered_at = NOW - 5 * DAY;
    let changes = decide_outcomes(
        &[make_rec(|r| r.notified_at = Some(delivered_at))],
        &inputs(|i| i.watched = movie_history(watched(Some(1.0), Some(delivered_at - 1)))),
    );
    assert!(changes.is_empty());
}

#[test]
fn does_not_infer_feedback_from_arr_removal() {
    let changes = decide_outcomes(
        &[make_rec(|r| {
            r.watchlist_result = Some(WatchlistResult::Added)
        })],
        &inputs(|_| {}),
    );
    assert!(changes.is_empty());
}

#[test]
fn labels_ignored_after_the_ignore_window() {
    let changes = decide_outcomes(
        &[make_rec(|r| {
            r.notified_at = Some(NOW - IGNORE_WINDOW_MS - 1000)
        })],
        &inputs(|_| {}),
    );
    assert_eq!(changes[0].status, RecommendationStatus::Ignored);
}

#[test]
fn leaves_in_progress_items_open_past_the_ignore_window() {
    let changes = decide_outcomes(
        &[make_rec(|r| {
            r.notified_at = Some(NOW - IGNORE_WINDOW_MS - 1000)
        })],
        &inputs(|i| {
            i.in_progress = movie_progress(ProgressState {
                progress: 0.4,
                last_viewed_at: None,
            });
        }),
    );
    assert!(changes.is_empty());
}

#[test]
fn does_not_treat_pre_recommendation_progress_as_current_engagement() {
    let delivered_at = NOW - IGNORE_WINDOW_MS - 1000;
    let changes = decide_outcomes(
        &[make_rec(|r| r.notified_at = Some(delivered_at))],
        &inputs(|i| {
            i.in_progress = movie_progress(ProgressState {
                progress: 0.4,
                last_viewed_at: Some(delivered_at - 1),
            });
        }),
    );
    assert_eq!(changes[0].status, RecommendationStatus::Ignored);
}

#[test]
fn suppresses_abandoned_when_the_in_progress_source_is_unavailable() {
    let changes = decide_outcomes(
        &[make_rec(|r| r.notified_at = Some(NOW - 20 * DAY))],
        &inputs(|i| {
            i.watched = movie_history(watched(Some(0.25), Some(NOW - ABANDONED_INACTIVITY_MS - 1)));
            i.in_progress_available = false;
        }),
    );
    assert!(changes.is_empty());
}

#[test]
fn suppresses_ignored_when_the_in_progress_source_is_unavailable() {
    let changes = decide_outcomes(
        &[make_rec(|r| {
            r.notified_at = Some(NOW - IGNORE_WINDOW_MS - 1000)
        })],
        &inputs(|i| i.in_progress_available = false),
    );
    assert!(changes.is_empty());
}

#[test]
fn still_labels_watched_when_absence_based_inputs_are_unavailable() {
    let changes = decide_outcomes(
        &[make_rec(|_| {})],
        &inputs(|i| {
            i.watched = movie_history(watched(Some(0.95), None));
            i.in_progress_available = false;
        }),
    );
    assert_eq!(changes[0].status, RecommendationStatus::Watched);
}

#[test]
fn ignores_rows_that_are_not_in_notified_status() {
    let changes = decide_outcomes(
        &[make_rec(|r| {
            r.status = RecommendationStatus::Pending;
            r.notified_at = Some(NOW - IGNORE_WINDOW_MS - 1000);
        })],
        &inputs(|_| {}),
    );
    assert!(changes.is_empty());
}
