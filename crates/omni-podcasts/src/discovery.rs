//! Tier-2 discovery: past-week web searches, then a cheap
//! model extracts a raw candidate list. Nothing here is verified; identities
//! and release dates come later from the shows' RSS feeds.

use futures::future::BoxFuture;
use omni_ai::ModelRole;
use omni_ai::tools::{SearchOptions, SearchTopic, TimeRange, WebSearchResult};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::log_file::{self, LogFile, code_block};
use crate::models::{Models, Refine};
use crate::pipeline::PipelineError;
use crate::sources::WebSearcher;
use crate::types::DiscoveredEpisode;

const LOG: &str = "PodcastRecsTask";
const MAX_DISCOVERED: usize = 12;

struct DiscoveryQuery {
    query: &'static str,
    topic: Option<SearchTopic>,
    time_range: TimeRange,
}

/// Multi-modal angles; each surfaces episodes the others miss.
const DISCOVERY_QUERIES: [DiscoveryQuery; 6] = [
    DiscoveryQuery {
        query: "best podcast episodes this week",
        topic: None,
        time_range: TimeRange::Week,
    },
    DiscoveryQuery {
        query: "reddit standout podcast episode this week discussion",
        topic: None,
        time_range: TimeRange::Week,
    },
    DiscoveryQuery {
        query: "podcast newsletter episode picks this week",
        topic: None,
        time_range: TimeRange::Week,
    },
    DiscoveryQuery {
        query: "new podcast episode interview philosophy history science media criticism",
        topic: None,
        time_range: TimeRange::Week,
    },
    DiscoveryQuery {
        query: "notable new podcast episode economics policy skepticism debate",
        topic: None,
        time_range: TimeRange::Week,
    },
    DiscoveryQuery {
        query: "podcast drama beef debate media gossip this week discussed",
        topic: Some(SearchTopic::News),
        time_range: TimeRange::Week,
    },
];

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExtractedEpisode {
    pub show_title: String,
    pub episode_title: String,
    /// One line: where/why this surfaced (thread, list, newsletter)
    pub context: String,
    pub source_url: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DiscoveryExtraction {
    pub episodes: Vec<ExtractedEpisode>,
}

impl Refine for DiscoveryExtraction {
    fn refine(&self) -> Result<(), String> {
        Ok(())
    }
}

pub(crate) fn format_results(results: &[WebSearchResult]) -> String {
    results
        .iter()
        .map(|r| {
            format!(
                "- {} ({})\n  {}",
                r.title,
                r.url,
                collapse_whitespace(&r.content)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `text.replace(/\s+/g, " ")`.
pub(crate) fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_space = false;
    for c in text.chars() {
        if c.is_whitespace() || c == '\u{feff}' {
            if !in_space {
                out.push(' ');
            }
            in_space = true;
        } else {
            out.push(c);
            in_space = false;
        }
    }
    out
}

pub async fn discover_episodes(
    web: &dyn WebSearcher,
    models: &Models,
    taste_digest: &str,
    recent_digest: &str,
    log_file: Option<&LogFile>,
) -> Result<Vec<DiscoveredEpisode>, PipelineError> {
    let searches: Vec<(&'static str, Vec<WebSearchResult>)> = crate::concurrency::buffered(DISCOVERY_QUERIES.iter().map(|q| Box::pin(async move {
        let options = SearchOptions {
            query: q.query.to_owned(),
            topic: q.topic,
            time_range: Some(q.time_range),
            max_results: Some(8),
            max_content_chars: Some(700),
        };
        match web.search(options).await {
            Ok(response) => (q.query, response.results),
            Err(error) => {
                tracing::warn!(target: LOG, error = %error, "Discovery search failed: {}", q.query);
                (q.query, Vec::new())
            }
        }
    }) as BoxFuture<'_, _>).collect(), 3)
    .await;

    let result_count: usize = searches.iter().map(|(_, r)| r.len()).sum();
    if result_count == 0 {
        tracing::warn!(target: LOG, "Discovery produced no search results");
        return Ok(Vec::new());
    }

    let model_id = models.model_id(ModelRole::RecsShortlist);
    let prompt = build_extraction_prompt(&searches, taste_digest, recent_digest);
    log_file::section(
        log_file,
        &format!("Discovery Extraction Prompt ({model_id})"),
        &code_block(&prompt, None),
    )
    .await?;
    tracing::info!(target: LOG, "Extracting candidates from {result_count} results ({model_id})");

    let generated = models
        .object::<DiscoveryExtraction>(ModelRole::RecsShortlist, "discover-episodes", prompt)
        .await
        .map_err(PipelineError::model("discover episodes"))?;
    tracing::info!(
        target: LOG,
        "Discovery token usage: {} prompt, {} completion",
        generated.usage.input_tokens,
        generated.usage.output_tokens
    );

    let mut seen = std::collections::HashSet::new();
    let mut episodes = Vec::new();
    for item in generated.output.episodes {
        let key = format!(
            "{}::{}",
            item.show_title.to_lowercase(),
            item.episode_title.to_lowercase()
        );
        if !seen.insert(key) {
            continue;
        }
        episodes.push(DiscoveredEpisode {
            show_title: item.show_title,
            episode_title: item.episode_title,
            context: item.context,
            source_url: item.source_url,
            matched_voices: None,
        });
        if episodes.len() >= MAX_DISCOVERED {
            break;
        }
    }
    let listing = episodes
        .iter()
        .map(|e| format!("- {} — {} ({})", e.show_title, e.episode_title, e.context))
        .collect::<Vec<_>>()
        .join("\n");
    log_file::section(
        log_file,
        "Discovered Episodes",
        if listing.is_empty() { "none" } else { &listing },
    )
    .await?;
    Ok(episodes)
}

fn build_extraction_prompt(
    searches: &[(&'static str, Vec<WebSearchResult>)],
    taste_digest: &str,
    recent_digest: &str,
) -> String {
    let blocks: Vec<String> = searches
        .iter()
        .filter(|(_, results)| !results.is_empty())
        .map(|(query, results)| format!("SEARCH: {query}\n{}", format_results(results)))
        .collect();
    format!(
        "You are extracting podcast episode candidates from web search results for one specific listener.

THE LISTENER:
{taste_digest}

{recent_digest}

From the search results below, extract up to {MAX_DISCOVERED} SPECIFIC podcast episodes (a show name AND an episode title/topic) that look like strong matches for this listener. Rules:
- Only episodes from shows the listener does NOT already subscribe to. The subscribed-show list above is an exclusion list AND taste evidence.
- Skip grifty, outrage-driven, influencer-style, true crime, lifestyle, and comedy/entertainment shows.
- Skip anything in the recently-recommended list.
- Prefer episodes with genuine discussion or curation behind them over algorithmic listicles.
- Episode titles may be approximate (they will be matched against the show's RSS feed later), but the show name must be as exact as possible.
- Do not invent episodes: every extraction must be traceable to a search result. Set source_url to the most relevant result URL, or null if none applies.

SEARCH RESULTS:
{}

Return JSON only.",
        blocks.join("\n\n")
    )
}
