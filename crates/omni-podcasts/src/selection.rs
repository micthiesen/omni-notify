//! Tier-2 selection (`selection.ts`): research each finalist once with
//! bounded web snippets, then choose one episode (or `no_add`) per call
//! against a shrinking finalist set.

use std::collections::HashMap;

use futures::future::BoxFuture;
use omni_ai::ModelRole;
use omni_ai::tools::SearchOptions;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::discovery::{collapse_whitespace, format_results};
use crate::js::to_date_stamp;
use crate::log_file::{self, LogFile, code_block};
use crate::models::{Models, Refine, in_range};
use crate::pipeline::PipelineError;
use crate::shortlist::ScoredEpisode;
use crate::sources::WebSearcher;

const LOG: &str = "PodcastRecsTask";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NotificationCopy {
    /// Short notification title prefixed with a topic emoji, e.g. '🏛️ The Rest Is Politics — Inside the Election'
    pub title: String,
    /// 2-3 sentence notification body: why this episode, for this listener
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PodcastSelectionPick {
    pub candidate_id: String,
    pub why_for_user: String,
    pub caveats: Vec<String>,
    pub confidence: f64,
    pub notification: NotificationCopy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DecisionKind {
    Select,
    NoAdd,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PodcastSelectionDecision {
    pub decision: DecisionKind,
    pub selected: Option<PodcastSelectionPick>,
    pub no_add_reason: Option<String>,
}

impl Refine for PodcastSelectionDecision {
    fn refine(&self) -> Result<(), String> {
        match &self.selected {
            Some(pick) => in_range(pick.confidence, 0.0, 1.0, "confidence"),
            None => Ok(()),
        }
    }
}

pub async fn select_episode(
    models: &Models,
    finalists: &[&ScoredEpisode],
    taste_digest: &str,
    research: &HashMap<String, String>,
    log_file: Option<&LogFile>,
) -> Result<PodcastSelectionDecision, PipelineError> {
    let model_id = models.model_id(ModelRole::RecsSelection);
    let prompt = build_prompt(finalists, taste_digest, research);
    log_file::section(
        log_file,
        &format!("Podcast Selection Prompt ({model_id})"),
        &code_block(&prompt, None),
    )
    .await?;
    tracing::info!(target: LOG, "Selecting from {} finalists ({model_id})", finalists.len());
    let generated = models
        .object::<PodcastSelectionDecision>(ModelRole::RecsSelection, "select", prompt)
        .await
        .map_err(PipelineError::model("select episode"))?;
    tracing::info!(
        target: LOG,
        "Selection token usage: {} prompt, {} completion",
        generated.usage.input_tokens,
        generated.usage.output_tokens
    );
    let json = serde_json::to_value(&generated.output)
        .map(|v| omni_core::js::json_stringify_pretty2(&v))
        .unwrap_or_default();
    log_file::section(
        log_file,
        "Podcast Selection Decision",
        &code_block(&json, Some("json")),
    )
    .await?;
    Ok(generated.output)
}

/// Bounded research per finalist; a failed search degrades to "no results".
pub async fn research_finalists(
    web: &dyn WebSearcher,
    finalists: &[ScoredEpisode],
    log_file: Option<&LogFile>,
) -> Result<HashMap<String, String>, PipelineError> {
    let entries: Vec<(String, String, String)> = crate::concurrency::buffered(finalists.iter().map(|finalist| Box::pin(async move {
        let candidate = &finalist.candidate;
        let query = format!(
            "\"{}\" podcast {} review discussion",
            candidate.show_title, candidate.episode_title
        );
        tracing::info!(target: LOG, "Selection research: {query}");
        let results = match web
            .search(SearchOptions {
                query,
                max_results: Some(3),
                max_content_chars: Some(800),
                ..SearchOptions::default()
            })
            .await
        {
            Ok(response) => response.results,
            Err(error) => {
                tracing::warn!(target: LOG, error = %error, "Research failed for {}", candidate.episode_title);
                Vec::new()
            }
        };
        let summary = format_results(&results);
        let heading = format!("Research: {} — {}", candidate.show_title, candidate.episode_title);
        (candidate.episode_id.clone(), heading, summary)
    }) as BoxFuture<'_, _>).collect(), 3)
    .await;
    let mut research = HashMap::new();
    for (episode_id, heading, summary) in entries {
        log_file::section(
            log_file,
            &heading,
            if summary.is_empty() {
                "No results"
            } else {
                &summary
            },
        )
        .await?;
        let value = if summary.is_empty() {
            "No research results available.".to_owned()
        } else {
            summary
        };
        research.insert(episode_id, value);
    }
    Ok(research)
}

fn build_prompt(
    finalists: &[&ScoredEpisode],
    taste_digest: &str,
    research: &HashMap<String, String>,
) -> String {
    let blocks: Vec<String> = finalists
        .iter()
        .map(|s| {
            let c = &s.candidate;
            let duration = c.duration_minutes.filter(|d| *d != 0).map(|d| format!(" | {d} min")).unwrap_or_default();
            let genres = if c.show_genres.is_empty() {
                "unknown genres".to_owned()
            } else {
                c.show_genres.join("/")
            };
            let risks = if s.risks.is_empty() {
                String::new()
            } else {
                format!("\n  Pre-screening risk flags: {}", s.risks.join("; "))
            };
            let description = omni_core::js::utf16_slice(&collapse_whitespace(&c.description), 0, 400).into_owned();
            let research = research
                .get(&c.episode_id)
                .cloned()
                .unwrap_or_else(|| "undefined".to_owned());
            format!(
                "[{}] {} — {} | {genres} | released {}{duration}\n  Surfaced via: {}{risks}\n  {description}\n  Research:\n{research}",
                c.episode_id,
                c.show_title,
                c.episode_title,
                to_date_stamp(c.published_at),
                c.discovered_via
            )
        })
        .collect();
    format!(
        "You are choosing at most ONE podcast episode to recommend to one person today. The listener LIKES having lots of options and would rather hear about a good episode than get nothing, so LEAN TOWARD RECOMMENDING: pick the best available finalist unless they are all genuinely weak or off-taste. A good, on-taste episode is worth surfacing even if it isn't a perfect standout — only skip when nothing here is a reasonable fit.

This is the TOPIC/standout tier. The listener's \"guest appearances of people I follow\" priority is handled by a SEPARATE stage — do NOT penalize a candidate here for lacking a followed voice. Judge purely on whether it's a solid, on-taste episode worth their time.

THE LISTENER (subscribed shows are ground truth; explicit feedback is direct preference evidence):
{taste_digest}

FINALISTS (identities and release dates already verified against each show's RSS feed):
{}

PROCESS:
1. Evaluate the research for genuine buzz vs promotional noise, host credibility, and whether the episode stands alone for a first-time listener of that show.
2. Compare against the listener's subscribed shows and feedback — justify against what they demonstrably listen to, not generic acclaim. A smart, on-taste episode from a show adjacent to what they love is a good pick even without manufactured drama.
3. Decide: select the strongest finalist. Use no_add ONLY when every finalist is genuinely weak or off-taste — not merely because none is a perfect standout, and never because a description doesn't prove sharp conflict.

Return the structured decision. Candidate ids must come from the list above. why_for_user should reference their actual listening patterns. Keep the notification concise and concrete, with a topic-appropriate emoji prefix on the title. For no_add, set selected to null and explain in no_add_reason.",
        blocks.join("\n\n")
    )
}
