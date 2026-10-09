//! Tier-1 discovery: recent episodes where a followed voice
//! guests. Both sources run per voice and are unioned: Podcast Index
//! `byperson` and a web person-search whose results a cheap model extracts.
//! A voice fails only when every configured source failed.

use std::collections::HashMap;

use futures::future::BoxFuture;
use omni_ai::ModelRole;
use omni_ai::tools::{SearchOptions, SearchTopic, TimeRange};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::account::PodcastAccount;
use crate::candidates::{podcast_index_to_candidate, resolve_candidates};
use crate::discovery::format_results;
use crate::filters::RECENT_EPISODE_WINDOW_MS;
use crate::log_file::{self, LogFile};
use crate::models::{Models, Refine};
use crate::pipeline::PipelineError;
use crate::podcastindex::PersonSearch;
use crate::sources::{ShowDirectory, WebSearcher};
use crate::types::{DiscoveredEpisode, EpisodeCandidate};

const LOG: &str = "PodcastRecsTask";
const VOICE_CONCURRENCY: usize = 3;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GuestEpisode {
    pub show_title: String,
    pub episode_title: String,
    pub source_url: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GuestExtraction {
    pub episodes: Vec<GuestEpisode>,
}

impl Refine for GuestExtraction {
    fn refine(&self) -> Result<(), String> {
        Ok(())
    }
}

/// Everything guest discovery reads from.
pub struct GuestSources<'a> {
    pub person_search: Option<&'a dyn PersonSearch>,
    pub web: &'a dyn WebSearcher,
    pub directory: &'a dyn ShowDirectory,
    pub account: Option<&'a dyn PodcastAccount>,
    pub models: &'a Models,
}

/// `voices` is the rotated per-run batch.
pub async fn discover_guest_appearances(
    sources: &GuestSources<'_>,
    voices: &[String],
    now: i64,
    log_file: Option<&LogFile>,
) -> Result<Vec<EpisodeCandidate>, PipelineError> {
    if voices.is_empty() {
        return Ok(Vec::new());
    }
    let cutoff = now - RECENT_EPISODE_WINDOW_MS;
    let per_voice: Vec<Result<Vec<EpisodeCandidate>, PipelineError>> =
        crate::concurrency::buffered(
            voices
                .iter()
                .map(|voice| {
                    Box::pin(discover_for_voice(sources, voice, cutoff)) as BoxFuture<'_, _>
                })
                .collect(),
            VOICE_CONCURRENCY,
        )
        .await;

    // One episode can feature several followed voices.
    let mut order: Vec<String> = Vec::new();
    let mut by_id: HashMap<String, EpisodeCandidate> = HashMap::new();
    for result in per_voice {
        for candidate in result? {
            match by_id.get_mut(&candidate.episode_id) {
                Some(existing) => {
                    let mut merged = existing.matched_voices.clone().unwrap_or_default();
                    for voice in candidate.matched_voices.unwrap_or_default() {
                        if !merged.contains(&voice) {
                            merged.push(voice);
                        }
                    }
                    existing.matched_voices = Some(merged);
                }
                None => {
                    order.push(candidate.episode_id.clone());
                    by_id.insert(candidate.episode_id.clone(), candidate);
                }
            }
        }
    }
    let candidates: Vec<EpisodeCandidate> =
        order.iter().filter_map(|id| by_id.remove(id)).collect();
    tracing::info!(
        target: LOG,
        "Guest discovery: {} candidate(s) across {} voice(s)",
        candidates.len(),
        voices.len()
    );
    let listing = candidates
        .iter()
        .map(|c| {
            format!(
                "- {} — {} [{}]",
                c.show_title,
                c.episode_title,
                c.matched_voices.clone().unwrap_or_default().join(", ")
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    log_file::section(
        log_file,
        "Guest Appearances",
        if listing.is_empty() { "none" } else { &listing },
    )
    .await?;
    Ok(candidates)
}

async fn discover_for_voice(
    sources: &GuestSources<'_>,
    voice: &str,
    cutoff: i64,
) -> Result<Vec<EpisodeCandidate>, PipelineError> {
    let (from_index, from_web) = futures::join!(
        via_podcast_index(sources.person_search, voice, cutoff),
        via_web(sources, voice)
    );
    let from_web = from_web?;
    if from_index.is_none() && from_web.is_none() {
        return Err(PipelineError::Invalid(format!(
            "Guest discovery failed for {voice}: all configured guest-discovery sources failed"
        )));
    }
    let mut all = from_index.unwrap_or_default();
    all.extend(from_web.unwrap_or_default());
    Ok(all)
}

async fn via_podcast_index(
    person_search: Option<&dyn PersonSearch>,
    voice: &str,
    cutoff: i64,
) -> Option<Vec<EpisodeCandidate>> {
    let search = person_search?;
    match search.search_by_person(voice).await {
        Ok(episodes) => Some(
            episodes
                .iter()
                .filter(|e| e.published_at >= cutoff)
                .filter_map(|e| podcast_index_to_candidate(e, voice))
                .collect(),
        ),
        Err(error) => {
            tracing::warn!(target: LOG, error = %error, "Podcast Index byperson failed for {voice}");
            None
        }
    }
}

/// `None` when the web search or extraction failed.
async fn via_web(
    sources: &GuestSources<'_>,
    voice: &str,
) -> Result<Option<Vec<EpisodeCandidate>>, PipelineError> {
    let response = match sources
        .web
        .search(SearchOptions {
            query: format!("\"{voice}\" podcast guest interview"),
            topic: Some(SearchTopic::News),
            time_range: Some(TimeRange::Week),
            max_results: Some(6),
            max_content_chars: Some(700),
        })
        .await
    {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(target: LOG, error = %error, "Tavily person-search failed for {voice}");
            return Ok(None);
        }
    };
    if response.results.is_empty() {
        return Ok(Some(Vec::new()));
    }
    let prompt = format!(
        "Recent web results for podcast episodes possibly featuring {voice} as a guest. Extract ONLY episodes where {voice} is actually a guest or participant (not merely mentioned or the topic). Give the podcast show name and episode title as precisely as you can.

RESULTS:
{}

Return JSON only; empty array if none clearly qualify.",
        format_results(&response.results)
    );
    let extraction = match sources
        .models
        .object::<GuestExtraction>(
            ModelRole::RecsShortlist,
            "extract-guest-appearances",
            prompt,
        )
        .await
    {
        Ok(generated) => generated.output,
        Err(error) => {
            tracing::warn!(target: LOG, error = %error, "Guest extraction failed for {voice}");
            return Ok(None);
        }
    };
    let discovered: Vec<DiscoveredEpisode> = extraction
        .episodes
        .into_iter()
        .map(|e| DiscoveredEpisode {
            show_title: e.show_title,
            episode_title: e.episode_title,
            context: format!("guest: {voice} (web)"),
            source_url: e.source_url,
            matched_voices: Some(vec![voice.to_owned()]),
        })
        .collect();
    if discovered.is_empty() {
        return Ok(Some(Vec::new()));
    }
    Ok(Some(
        resolve_candidates(&discovered, sources.account, sources.directory, None).await?,
    ))
}
