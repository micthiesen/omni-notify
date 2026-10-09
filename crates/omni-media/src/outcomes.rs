//! Passive outcome labels for delivered recommendations (`src/recommendations/outcomes.ts`).

use std::collections::HashMap;

use omni_api::media::RecommendationStatus;

use crate::js::to_fixed;
use crate::persistence::RecommendationData;
use crate::types::MediaType;

/// Fraction of runtime that counts as having watched a title.
pub const WATCHED_COMPLETION_THRESHOLD: f64 = 0.8;
const DAY_MS: i64 = 24 * 60 * 60 * 1000;
/// Time after notification with no engagement before a rec counts as ignored.
pub const IGNORE_WINDOW_MS: i64 = 30 * DAY_MS;
pub const ABANDONED_INACTIVITY_MS: i64 = 14 * DAY_MS;

/// Watch-history state of one canonical title.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WatchedState {
    /// 0-1 when known; `None` means the backend reports only a view.
    pub completion: Option<f64>,
    pub view_count: i64,
    pub last_viewed_at: Option<i64>,
}

/// In-progress state of one canonical title.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ProgressState {
    pub progress: f64,
    pub last_viewed_at: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct OutcomeInputs {
    pub watched: HashMap<String, WatchedState>,
    pub in_progress: HashMap<String, ProgressState>,
    /// False when the in-progress source was unavailable: labels inferred
    /// from the absence of progress (abandoned, ignored) are suppressed.
    pub in_progress_available: bool,
    pub now: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OutcomeChange {
    pub recommendation_id: String,
    pub canonical_id: String,
    pub status: RecommendationStatus,
    pub reason: String,
}

/// Labels notified recommendations from polled state:
/// - watched: completion at/above the threshold after delivery, or a movie
///   view when the backend reports no progress;
/// - abandoned: started, stalled below the threshold for two weeks, and no
///   longer in progress;
/// - ignored: the ignore window elapsed with no engagement.
///
/// These are bookkeeping for cooldowns and the UI; explicit feedback is the
/// preference signal. Arr removals are never treated as feedback.
pub fn decide_outcomes(open: &[RecommendationData], inputs: &OutcomeInputs) -> Vec<OutcomeChange> {
    let mut changes = Vec::new();
    for rec in open {
        if rec.status() != RecommendationStatus::Notified {
            continue;
        }
        let canonical_id = &rec.canonical_id;
        let history = inputs.watched.get(canonical_id);
        let delivered_at = rec.delivered_at();
        let progress = inputs
            .in_progress
            .get(canonical_id)
            .filter(|p| p.last_viewed_at.is_none_or(|at| at >= delivered_at));
        let engaged_after_delivery =
            history.is_some_and(|h| h.last_viewed_at.is_none_or(|at| at >= delivered_at));

        if let Some(history) = history.filter(|_| engaged_after_delivery) {
            let watched = match history.completion {
                None => rec.media_type == MediaType::Movie && history.view_count >= 1,
                Some(completion) => completion >= WATCHED_COMPLETION_THRESHOLD,
            };
            if watched {
                changes.push(OutcomeChange {
                    recommendation_id: rec.recommendation_id.clone(),
                    canonical_id: canonical_id.clone(),
                    status: RecommendationStatus::Watched,
                    reason: match history.completion {
                        None => format!("viewCount={}", history.view_count),
                        Some(completion) => format!("completion={}", to_fixed(completion, 2)),
                    },
                });
                continue;
            }
        }

        let stalled = history.filter(|h| {
            inputs.in_progress_available
                && engaged_after_delivery
                && h.completion
                    .is_some_and(|completion| completion < WATCHED_COMPLETION_THRESHOLD)
                && h.last_viewed_at
                    .is_some_and(|at| inputs.now - at >= ABANDONED_INACTIVITY_MS)
                && progress.is_none()
        });
        if let Some(history) = stalled {
            changes.push(OutcomeChange {
                recommendation_id: rec.recommendation_id.clone(),
                canonical_id: canonical_id.clone(),
                status: RecommendationStatus::Abandoned,
                reason: format!(
                    "stalled at {}%",
                    to_fixed(history.completion.unwrap_or(0.0) * 100.0, 0)
                ),
            });
            continue;
        }

        // Still in progress, or unknowable: leave open regardless of age.
        if progress.is_some() || !inputs.in_progress_available {
            continue;
        }
        if inputs.now - delivered_at > IGNORE_WINDOW_MS {
            changes.push(OutcomeChange {
                recommendation_id: rec.recommendation_id.clone(),
                canonical_id: canonical_id.clone(),
                status: RecommendationStatus::Ignored,
                reason: "no engagement within 30 days".to_owned(),
            });
        }
    }
    changes
}
