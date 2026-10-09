//! Resolution of discovered episodes into verified candidates:
//! show identity via iTunes (Castro search as fallback),
//! then the episode and its authoritative release date from the show's own
//! RSS feed. Anything unverifiable is dropped with a logged reason.

use std::collections::HashSet;

use futures::future::BoxFuture;

use crate::account::PodcastAccount;
use crate::itunes::pick_best_show_match;
use crate::log_file::{self, LogFile, LogFileError};
use crate::podcastindex::PodcastIndexEpisode;
use crate::rss::find_episode_by_title;
use crate::sources::ShowDirectory;
use crate::titles::normalize_title;
use crate::types::{DiscoveredEpisode, EpisodeCandidate, make_episode_id, make_show_id};

const LOG: &str = "PodcastRecsTask";
const RESOLVE_CONCURRENCY: usize = 3;
const FEED_EPISODES: usize = 30;

struct ResolvedShow {
    title: String,
    feed_url: String,
    itunes_id: Option<i64>,
    artwork_url: Option<String>,
    genres: Vec<String>,
}

pub async fn resolve_candidates(
    discovered: &[DiscoveredEpisode],
    account: Option<&dyn PodcastAccount>,
    directory: &dyn ShowDirectory,
    log_file: Option<&LogFile>,
) -> Result<Vec<EpisodeCandidate>, LogFileError> {
    let resolved: Vec<Result<EpisodeCandidate, String>> = crate::concurrency::buffered(
        discovered
            .iter()
            .map(|item| Box::pin(resolve_one(item, account, directory)) as BoxFuture<'_, _>)
            .collect(),
        RESOLVE_CONCURRENCY,
    )
    .await;

    let mut dropped = Vec::new();
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();
    for (item, result) in discovered.iter().zip(resolved) {
        match result {
            Err(reason) => dropped.push(format!(
                "- {} — {}: {reason}",
                item.show_title, item.episode_title
            )),
            Ok(candidate) => {
                if seen.insert(candidate.episode_id.clone()) {
                    candidates.push(candidate);
                }
            }
        }
    }
    tracing::info!(
        target: LOG,
        "Resolved {}/{} discovered episodes",
        candidates.len(),
        discovered.len()
    );
    if !dropped.is_empty() {
        log_file::section(log_file, "Resolution Failures", &dropped.join("\n")).await?;
    }
    Ok(candidates)
}

async fn resolve_one(
    item: &DiscoveredEpisode,
    account: Option<&dyn PodcastAccount>,
    directory: &dyn ShowDirectory,
) -> Result<EpisodeCandidate, String> {
    let show = resolve_show(&item.show_title, account, directory)
        .await?
        .ok_or_else(|| "show not found on iTunes or Castro".to_owned())?;
    let episodes = directory.fetch_feed(&show.feed_url, FEED_EPISODES).await?;
    let episode = find_episode_by_title(&episodes, &item.episode_title)
        .ok_or_else(|| "episode not found in RSS feed".to_owned())?;
    let show_id = make_show_id(show.itunes_id, Some(&show.feed_url))
        .ok_or_else(|| "could not build canonical show id".to_owned())?;
    Ok(EpisodeCandidate {
        episode_id: make_episode_id(&show_id, &episode.guid),
        show_id,
        show_title: show.title,
        episode_title: episode.title.clone(),
        feed_url: show.feed_url,
        itunes_id: show.itunes_id,
        artwork_url: show.artwork_url,
        episode_guid: episode.guid.clone(),
        media_url: episode.enclosure_url.clone(),
        episode_url: episode.link.clone(),
        published_at: episode.published_at,
        duration_minutes: episode.duration_minutes,
        description: episode.description.clone(),
        show_genres: show.genres,
        discovered_via: item.context.clone(),
        source_url: item.source_url.clone(),
        matched_voices: item.matched_voices.clone(),
    })
}

/// iTunes first; Castro's search catches private and niche feeds (no genres).
async fn resolve_show(
    show_title: &str,
    account: Option<&dyn PodcastAccount>,
    directory: &dyn ShowDirectory,
) -> Result<Option<ResolvedShow>, String> {
    let shows = directory.search_itunes(show_title).await?;
    if let Some(show) = pick_best_show_match(&shows, show_title)
        && let Some(feed_url) = show.feed_url.clone().filter(|f| !f.is_empty())
    {
        return Ok(Some(ResolvedShow {
            title: show.title.clone(),
            feed_url,
            itunes_id: Some(show.itunes_id),
            artwork_url: show.artwork_url.clone(),
            genres: show.genres.clone(),
        }));
    }
    let Some(account) = account else {
        return Ok(None);
    };
    let Ok(results) = account.search_podcasts(show_title).await else {
        return Ok(None);
    };
    let Some(matched) = pick_best_by_title(&results, show_title, |r| &r.title) else {
        return Ok(None);
    };
    if matched.feed_url.is_empty() {
        return Ok(None);
    }
    Ok(Some(ResolvedShow {
        title: matched.title.clone(),
        feed_url: matched.feed_url.clone(),
        itunes_id: matched.itunes_id,
        artwork_url: matched.artwork_url.clone(),
        genres: Vec::new(),
    }))
}

/// A Podcast Index episode is already resolved; it maps straight to a
/// candidate tagged with the voice whose byperson search surfaced it.
pub fn podcast_index_to_candidate(
    episode: &PodcastIndexEpisode,
    voice: &str,
) -> Option<EpisodeCandidate> {
    let show_id = make_show_id(episode.feed_itunes_id, Some(&episode.feed_url))?;
    Some(EpisodeCandidate {
        episode_id: make_episode_id(&show_id, &episode.guid),
        show_id,
        show_title: episode.feed_title.clone(),
        episode_title: episode.title.clone(),
        feed_url: episode.feed_url.clone(),
        itunes_id: episode.feed_itunes_id,
        artwork_url: episode.artwork_url.clone(),
        episode_guid: episode.guid.clone(),
        media_url: Some(episode.enclosure_url.clone()),
        episode_url: episode.episode_url.clone(),
        published_at: episode.published_at,
        duration_minutes: episode.duration_minutes,
        description: episode.description.clone(),
        show_genres: Vec::new(),
        discovered_via: format!("guest: {voice} (Podcast Index)"),
        source_url: None,
        matched_voices: Some(vec![voice.to_owned()]),
    })
}

/// Loose title match for the Castro fallback: exact, then containment.
pub fn pick_best_by_title<'a, T>(
    items: &'a [T],
    show_title: &str,
    title: impl Fn(&T) -> &str,
) -> Option<&'a T> {
    let target = normalize_title(show_title);
    items
        .iter()
        .find(|item| normalize_title(title(item)) == target)
        .or_else(|| {
            items.iter().find(|item| {
                let candidate = normalize_title(title(item));
                candidate.contains(&target) || target.contains(&candidate)
            })
        })
}
