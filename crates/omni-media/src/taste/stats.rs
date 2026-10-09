//! Deterministic behavioral stats.

use std::cmp::Ordering;

use indexmap::{IndexMap, IndexSet};
use omni_api::media::{
    FeedbackStats, MediaType, OrderedMap, RecommendationFeedback, RecommendationOutcomeStats,
    RecommendationStatus, SourcePerformance, TasteBehaviorStats,
};
use omni_core::js::locale_compare;

use super::types::{TasteEvidenceData, TasteEvidenceKind};

const HOUR_MS: f64 = 60.0 * 60.0 * 1000.0;

pub fn compute_behavioral_stats(evidence: &[TasteEvidenceData]) -> TasteBehaviorStats {
    let latest_watch = latest_by_canonical(
        evidence
            .iter()
            .filter(|item| item.kind == TasteEvidenceKind::PlexWatch),
    );
    let latest_outcome = latest_recommendation_evidence(
        evidence
            .iter()
            .filter(|item| item.kind == TasteEvidenceKind::RecommendationOutcome),
    );
    let latest_feedback = latest_recommendation_evidence(
        evidence
            .iter()
            .filter(|item| item.kind == TasteEvidenceKind::ExplicitFeedback),
    );
    let delivered: IndexSet<&str> = latest_outcome
        .iter()
        .filter(|(_, item)| is_delivered(item.recommendation_status))
        .map(|(id, _)| *id)
        .collect();
    #[allow(clippy::cast_precision_loss)]
    let hours_to_start: Vec<f64> = latest_outcome
        .values()
        .filter_map(|item| {
            let (started, recommended) = (item.started_at?, item.recommended_at?);
            (is_delivered(item.recommendation_status)
                && item
                    .recommendation_id
                    .as_deref()
                    .is_some_and(|id| !id.is_empty())
                && started >= recommended)
                .then(|| (started - recommended) as f64 / HOUR_MS)
        })
        .collect();

    let mut sources: IndexMap<&str, &str> = IndexMap::new();
    for item in evidence {
        if let (Some(id), Some(source)) = (
            item.recommendation_id
                .as_deref()
                .filter(|id| !id.is_empty()),
            item.source.as_deref().filter(|s| !s.is_empty()),
        ) {
            sources.insert(id, source);
        }
    }
    let mut source_performance: OrderedMap<SourcePerformance> = OrderedMap::default();
    for id in &delivered {
        let source = sources.get(id).copied().unwrap_or("unknown");
        if source_performance.get(source).is_none() {
            source_performance.insert(source.to_owned(), SourcePerformance::default());
        }
        let Some(row) = source_performance.get_mut(source) else {
            continue;
        };
        row.total += 1;
        if latest_outcome
            .get(id)
            .is_some_and(|o| o.recommendation_status == Some(RecommendationStatus::Watched))
        {
            row.watched += 1;
        }
        match latest_feedback.get(id).and_then(|f| f.feedback) {
            Some(RecommendationFeedback::GoodPick) => row.good_pick += 1,
            Some(RecommendationFeedback::NotForMe) => row.not_for_me += 1,
            _ => {}
        }
    }

    let count_watch = |filter: &dyn Fn(&TasteEvidenceData) -> bool| {
        latest_watch.values().filter(|item| filter(item)).count() as u64
    };
    let count_status = |status: RecommendationStatus| {
        latest_outcome
            .values()
            .filter(|item| item.recommendation_status == Some(status))
            .count() as u64
    };
    let count_feedback = |feedback: RecommendationFeedback| {
        latest_feedback
            .values()
            .filter(|item| item.feedback == Some(feedback))
            .count() as u64
    };
    let awaiting = latest_outcome
        .iter()
        .filter(|(id, item)| {
            item.recommendation_status == Some(RecommendationStatus::Notified)
                && !matches!(
                    latest_feedback.get(*id).and_then(|f| f.feedback),
                    Some(RecommendationFeedback::NotForMe | RecommendationFeedback::AlreadyWatched)
                )
        })
        .count() as u64;

    #[allow(clippy::cast_precision_loss)]
    let average_hours_to_start = (!hours_to_start.is_empty())
        .then(|| hours_to_start.iter().sum::<f64>() / hours_to_start.len() as f64);

    TasteBehaviorStats {
        completed_movies: count_watch(&|item| {
            item.media_type == MediaType::Movie && is_completed_watch(item)
        }),
        completed_series: count_watch(&|item| {
            item.media_type == MediaType::Tv && is_completed_watch(item)
        }),
        rewatched_titles: count_watch(&|item| {
            is_completed_watch(item) && item.view_count.unwrap_or(1) > 1
        }),
        recommendations: RecommendationOutcomeStats {
            total: delivered.len() as u64,
            watched: count_status(RecommendationStatus::Watched),
            abandoned: count_status(RecommendationStatus::Abandoned),
            ignored: count_status(RecommendationStatus::Ignored),
            failed: count_status(RecommendationStatus::Failed),
            awaiting_outcome: awaiting,
        },
        feedback: FeedbackStats {
            good_pick: count_feedback(RecommendationFeedback::GoodPick),
            not_for_me: count_feedback(RecommendationFeedback::NotForMe),
            already_watched: count_feedback(RecommendationFeedback::AlreadyWatched),
        },
        average_hours_to_start,
        source_performance,
    }
}

fn is_delivered(status: Option<RecommendationStatus>) -> bool {
    matches!(
        status,
        Some(
            RecommendationStatus::Notified
                | RecommendationStatus::Watched
                | RecommendationStatus::Abandoned
                | RecommendationStatus::Ignored
        )
    )
}

/// Completed by completion when known, else a movie with a view.
pub fn is_completed_watch(item: &TasteEvidenceData) -> bool {
    match item.completion {
        Some(completion) => completion >= 0.8,
        None => item.media_type == MediaType::Movie && item.view_count.unwrap_or(0) >= 1,
    }
}

fn latest_by_canonical<'a>(
    items: impl Iterator<Item = &'a TasteEvidenceData>,
) -> IndexMap<&'a str, &'a TasteEvidenceData> {
    let mut latest: IndexMap<&str, &TasteEvidenceData> = IndexMap::new();
    for item in items {
        let replace = latest.get(item.canonical_id.as_str()).is_none_or(|prior| {
            item.observed_at > prior.observed_at
                || (item.observed_at == prior.observed_at
                    && locale_compare(&item.evidence_id, &prior.evidence_id) == Ordering::Greater)
        });
        if replace {
            latest.insert(item.canonical_id.as_str(), item);
        }
    }
    latest
}

fn status_precedence(item: &TasteEvidenceData) -> u8 {
    match item.recommendation_status {
        Some(
            RecommendationStatus::Watched
            | RecommendationStatus::Abandoned
            | RecommendationStatus::Ignored,
        ) => 4,
        Some(RecommendationStatus::Failed) => 3,
        Some(RecommendationStatus::Notified) => 2,
        Some(RecommendationStatus::Pending) => 1,
        None => 0,
    }
}

fn latest_recommendation_evidence<'a>(
    items: impl Iterator<Item = &'a TasteEvidenceData>,
) -> IndexMap<&'a str, &'a TasteEvidenceData> {
    let mut latest: IndexMap<&str, &TasteEvidenceData> = IndexMap::new();
    for item in items {
        let Some(id) = item
            .recommendation_id
            .as_deref()
            .filter(|id| !id.is_empty())
        else {
            continue;
        };
        let replace = latest.get(id).is_none_or(|prior| {
            item.observed_at > prior.observed_at
                || (item.observed_at == prior.observed_at
                    && (status_precedence(item) > status_precedence(prior)
                        || (status_precedence(item) == status_precedence(prior)
                            && locale_compare(&item.evidence_id, &prior.evidence_id)
                                == Ordering::Greater)))
        });
        if replace {
            latest.insert(id, item);
        }
    }
    latest
}
