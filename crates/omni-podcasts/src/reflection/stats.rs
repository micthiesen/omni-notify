//! Deterministic behavioral counts (`reflection/stats.ts`). Listens dedupe on
//! (show, episode) and recommendation rows on `recommendationId`, newest wins.

use std::collections::{HashMap, HashSet};

use super::types::{
    FeedbackCounts, PodcastBehavioralStats, PodcastTasteEvidenceData, PodcastTasteEvidenceKind,
    RecommendationCounts,
};
use crate::persistence::{PodcastFeedback, PodcastRecommendationStatus};

fn keep_newest<'a>(
    map: &mut HashMap<String, &'a PodcastTasteEvidenceData>,
    key: String,
    item: &'a PodcastTasteEvidenceData,
) {
    if map
        .get(&key)
        .is_none_or(|existing| item.observed_at > existing.observed_at)
    {
        map.insert(key, item);
    }
}

pub fn compute_podcast_behavioral_stats(
    evidence: &[PodcastTasteEvidenceData],
) -> PodcastBehavioralStats {
    let mut listens = HashMap::new();
    let mut outcomes = HashMap::new();
    let mut feedback = HashMap::new();
    for item in evidence {
        if item.kind == PodcastTasteEvidenceKind::Listen {
            let key = format!(
                "{}#{}",
                item.show_key,
                item.episode_title.as_deref().unwrap_or_default()
            );
            keep_newest(&mut listens, key, item);
        } else if let Some(id) = item
            .recommendation_id
            .as_deref()
            .filter(|id| !id.is_empty())
        {
            if item.kind == PodcastTasteEvidenceKind::RecommendationOutcome {
                keep_newest(&mut outcomes, id.to_owned(), item);
            } else {
                keep_newest(&mut feedback, id.to_owned(), item);
            }
        }
    }
    let count = |map: &HashMap<String, &PodcastTasteEvidenceData>,
                 f: &dyn Fn(&PodcastTasteEvidenceData) -> bool| {
        map.values().filter(|item| f(item)).count() as u64
    };
    let status = |s: PodcastRecommendationStatus| {
        count(&outcomes, &|item| item.recommendation_status == Some(s))
    };
    PodcastBehavioralStats {
        listened_episodes: count(&listens, &|item| item.completion.is_none_or(|c| c >= 0.8)),
        started_episodes: listens.len() as u64,
        starred_episodes: count(&listens, &|item| item.starred == Some(true)),
        distinct_shows: evidence
            .iter()
            .map(|e| e.show_key.as_str())
            .collect::<HashSet<_>>()
            .len() as u64,
        recommendations: RecommendationCounts {
            total: outcomes.len() as u64,
            listened: status(PodcastRecommendationStatus::Listened),
            abandoned: status(PodcastRecommendationStatus::Abandoned),
            ignored: status(PodcastRecommendationStatus::Ignored),
            failed: status(PodcastRecommendationStatus::Failed),
            awaiting_outcome: status(PodcastRecommendationStatus::Pending)
                + status(PodcastRecommendationStatus::Notified),
        },
        feedback: FeedbackCounts {
            good_pick: count(&feedback, &|item| {
                item.feedback == Some(PodcastFeedback::GoodPick)
            }),
            not_for_me: count(&feedback, &|item| {
                item.feedback == Some(PodcastFeedback::NotForMe)
            }),
        },
    }
}
