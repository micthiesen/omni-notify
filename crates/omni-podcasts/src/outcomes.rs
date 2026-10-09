//! Passive outcome labels for delivered recommendations from listen history.
//! Only called when history is actually available.

use std::collections::HashMap;

use omni_core::js::to_fixed;

use crate::account::ListenedEpisode;
use crate::castro::client::normalize_media_url;
use crate::persistence::{PodcastRecommendationData, PodcastRecommendationStatus};
use crate::titles::normalize_title;

/// Fraction of an episode that counts as having listened to it.
pub const LISTENED_COMPLETION_THRESHOLD: f64 = 0.8;
/// No engagement this long after notification counts as ignored.
pub const IGNORE_WINDOW_MS: i64 = 30 * 24 * 60 * 60 * 1000;
pub const ABANDONED_INACTIVITY_MS: i64 = 14 * 24 * 60 * 60 * 1000;

#[derive(Clone, Debug, PartialEq)]
pub struct PodcastOutcomeChange {
    pub recommendation_id: String,
    pub episode_id: String,
    /// Listened, Abandoned or Ignored.
    pub status: PodcastRecommendationStatus,
    pub reason: String,
}

fn titles_key(show_title: &str, episode_title: &str) -> String {
    format!(
        "{}::{}",
        normalize_title(show_title),
        normalize_title(episode_title)
    )
}

fn keep_newest<'a>(
    map: &mut HashMap<String, &'a ListenedEpisode>,
    key: String,
    item: &'a ListenedEpisode,
) {
    let newer = map
        .get(&key)
        .is_none_or(|prior| item.listened_at > prior.listened_at);
    if newer {
        map.insert(key, item);
    }
}

pub fn decide_episode_outcomes(
    open: &[PodcastRecommendationData],
    history: &[ListenedEpisode],
    now: i64,
) -> Vec<PodcastOutcomeChange> {
    let mut by_guid: HashMap<String, &ListenedEpisode> = HashMap::new();
    let mut by_media: HashMap<String, &ListenedEpisode> = HashMap::new();
    let mut by_titles: HashMap<String, &ListenedEpisode> = HashMap::new();
    for item in history {
        if let Some(guid) = item.episode_guid.as_deref().filter(|g| !g.is_empty()) {
            keep_newest(&mut by_guid, guid.to_owned(), item);
        }
        if let Some(media) = normalize_media_url(item.media_url.as_deref()) {
            keep_newest(&mut by_media, media, item);
        }
        keep_newest(
            &mut by_titles,
            titles_key(&item.show_title, &item.episode_title),
            item,
        );
    }

    let mut changes = Vec::new();
    for rec in open {
        if rec.status != PodcastRecommendationStatus::Notified {
            continue;
        }
        let delivered_at = rec.notified_at.unwrap_or(rec.recommended_at);
        let listened = by_guid
            .get(&rec.episode_guid)
            .or_else(|| {
                by_media.get(&normalize_media_url(rec.media_url.as_deref()).unwrap_or_default())
            })
            .or_else(|| by_titles.get(&titles_key(&rec.show_title, &rec.episode_title)))
            .copied();
        let engaged = listened.filter(|l| l.listened_at >= delivered_at);

        if let Some(l) = engaged
            && l.completion
                .is_none_or(|c| c >= LISTENED_COMPLETION_THRESHOLD)
        {
            changes.push(PodcastOutcomeChange {
                recommendation_id: rec.recommendation_id.clone(),
                episode_id: rec.episode_id.clone(),
                status: PodcastRecommendationStatus::Listened,
                reason: match l.completion {
                    None => "playback recorded".to_owned(),
                    Some(c) => format!("completion={}", to_fixed(c, 2)),
                },
            });
            continue;
        }

        if let Some(l) = engaged
            && let Some(c) = l.completion
            && c < LISTENED_COMPLETION_THRESHOLD
            && now - l.listened_at >= ABANDONED_INACTIVITY_MS
        {
            changes.push(PodcastOutcomeChange {
                recommendation_id: rec.recommendation_id.clone(),
                episode_id: rec.episode_id.clone(),
                status: PodcastRecommendationStatus::Abandoned,
                reason: format!("stalled at {}%", to_fixed(c * 100.0, 0)),
            });
            continue;
        }

        if engaged.is_none() && now - delivered_at > IGNORE_WINDOW_MS {
            changes.push(PodcastOutcomeChange {
                recommendation_id: rec.recommendation_id.clone(),
                episode_id: rec.episode_id.clone(),
                status: PodcastRecommendationStatus::Ignored,
                reason: "no engagement within 30 days".to_owned(),
            });
        }
    }
    changes
}
