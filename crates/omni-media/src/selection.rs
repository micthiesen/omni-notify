//! Research the finalists once, then pick one title or `no_add`.

use std::collections::HashMap;

use futures::StreamExt;
use futures::future::BoxFuture;
use omni_ai::tools::{SearchOptions, WebSearch};
use omni_ai::{Ai, CostTag, GenerateRequest, LanguageModel, ModelRole};
use omni_core::js::{number_to_string, to_fixed};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::IntegrationError;
use crate::js::code_block;
use crate::js::collapse_whitespace;
use crate::js::slice_utf16;
use crate::run_log::{self, RunLogFile};
use crate::shortlist::{ScoredCandidate, format_candidate_details};

const LOG: &str = "Main:RecsTask";
const RESEARCH_CONCURRENCY: usize = 4;

/// One research hit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResearchHit {
    pub title: String,
    pub url: String,
    pub content: String,
}

/// Bounded web research (Tavily in production).
pub trait Research: Send + Sync {
    fn search<'a>(
        &'a self,
        query: &'a str,
        max_results: u32,
        max_content_chars: usize,
    ) -> BoxFuture<'a, Result<Vec<ResearchHit>, String>>;
}

impl Research for WebSearch {
    fn search<'a>(
        &'a self,
        query: &'a str,
        max_results: u32,
        max_content_chars: usize,
    ) -> BoxFuture<'a, Result<Vec<ResearchHit>, String>> {
        Box::pin(async move {
            let results = WebSearch::search(
                self,
                SearchOptions {
                    query: query.to_owned(),
                    max_results: Some(max_results),
                    max_content_chars: Some(max_content_chars),
                    ..SearchOptions::default()
                },
            )
            .await
            .map_err(|e| omni_core::error::chain_message(&e))?;
            Ok(results
                .results
                .into_iter()
                .map(|r| ResearchHit {
                    title: r.title,
                    url: r.url,
                    content: r.content,
                })
                .collect())
        })
    }
}

/// Notification copy written by the selector.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NotificationCopy {
    /// Short notification title prefixed with an emoji, e.g. '🎬 Dune (2021)'
    pub title: String,
    /// 2-3 sentence notification body: why this pick, for this user
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SelectionPick {
    pub candidate_id: String,
    pub why_for_user: String,
    pub caveats: Vec<String>,
    #[schemars(range(min = 0, max = 1))]
    pub confidence: f64,
    pub notification: NotificationCopy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Select,
    NoAdd,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SelectionDecision {
    pub decision: Decision,
    pub selected: Option<SelectionPick>,
    /// Second choice with its OWN reasoning and notification copy; used only if the first is already on the watchlist
    pub backup: Option<SelectionPick>,
    pub no_add_reason: Option<String>,
}

impl SelectionDecision {
    fn validate(&self) -> Result<(), String> {
        for pick in [&self.selected, &self.backup].into_iter().flatten() {
            if !(0.0..=1.0).contains(&pick.confidence) {
                return Err(format!(
                    "No object generated: confidence for {} is out of range",
                    pick.candidate_id
                ));
            }
        }
        Ok(())
    }
}

/// Researches each finalist once (bounded snippets), keyed by canonical id.
pub async fn research_finalists(
    research: &dyn Research,
    finalists: &[ScoredCandidate],
    log_file: Option<&RunLogFile>,
) -> Result<HashMap<String, String>, IntegrationError> {
    let entries: Vec<Result<(String, String), IntegrationError>> = futures::stream::iter(
        finalists
            .iter()
            .map(|scored| async move {
                let candidate = &scored.candidate;
                let year = candidate.year.map(|y| y.to_string()).unwrap_or_default();
                let query = format!(
                    "{} {year} critical reception audience reviews ending quality",
                    candidate.title
                );
                tracing::info!(target: LOG, "Selection research: {query}");
                let hits = match research.search(&query, 3, 900).await {
                    Ok(hits) => hits,
                    Err(error) => {
                        tracing::warn!(
                            target: LOG,
                            error = %error,
                            "Research failed for {}",
                            candidate.title
                        );
                        Vec::new()
                    }
                };
                let summary = hits
                    .iter()
                    .map(|hit| {
                        format!(
                            "- {} ({})\n  {}",
                            hit.title,
                            hit.url,
                            collapse_whitespace(&hit.content)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let logged = if summary.is_empty() {
                    "No results"
                } else {
                    &summary
                };
                run_log::section(log_file, &format!("Research: {}", candidate.title), logged)
                    .await?;
                let value = if summary.is_empty() {
                    "No research results available.".to_owned()
                } else {
                    summary
                };
                Ok((candidate.canonical_id.clone(), value))
            })
            .collect::<Vec<_>>(),
    )
    .buffered(RESEARCH_CONCURRENCY)
    .collect()
    .await;
    entries.into_iter().collect()
}

/// One selection call over the remaining finalists.
pub async fn select_recommendation(
    ai: &Ai,
    model: &dyn LanguageModel,
    finalists: &[ScoredCandidate],
    history_digest: &str,
    research: &HashMap<String, String>,
    log_file: Option<&RunLogFile>,
) -> Result<SelectionDecision, IntegrationError> {
    let model_id = model.id().to_string();
    let prompt = build_prompt(finalists, history_digest, research);
    if let Some(log) = log_file {
        log.section(
            &format!("Selection Prompt ({model_id})"),
            &code_block(&prompt, None),
        )
        .await?;
        tracing::info!(
            target: LOG,
            "Selecting from {} finalists ({model_id})",
            finalists.len()
        );
    }
    let operation = "generate recommendation selection";
    let (decision, usage) = ai
        .generate_object::<SelectionDecision>(
            model,
            GenerateRequest::prompt(prompt),
            CostTag::for_role(ModelRole::RecsSelection),
        )
        .await
        .map_err(|e| IntegrationError::from_error(operation, &e))?;
    decision
        .validate()
        .map_err(|e| IntegrationError::new(operation, e))?;
    tracing::info!(
        target: LOG,
        "Selection token usage: {} prompt, {} completion",
        usage.input_tokens,
        usage.output_tokens
    );
    if log_file.is_some() {
        let json = serde_json::to_value(&decision)
            .map(|v| omni_core::js::json_stringify_pretty2(&v))
            .unwrap_or_default();
        run_log::section(
            log_file,
            "Selection Decision",
            &code_block(&json, Some("json")),
        )
        .await?;
    }
    Ok(decision)
}

fn build_prompt(
    finalists: &[ScoredCandidate],
    history_digest: &str,
    research: &HashMap<String, String>,
) -> String {
    let blocks: Vec<String> = finalists
        .iter()
        .map(|s| {
            let c = &s.candidate;
            let year = c
                .year
                .filter(|y| *y != 0)
                .map(|y| format!(" ({y})"))
                .unwrap_or_default();
            let genres = if c.genres.is_empty() {
                "unknown genres".to_owned()
            } else {
                c.genres.join("/")
            };
            let library = if c.in_library {
                "\n  Already available in the local library."
            } else {
                ""
            };
            let risks = if s.risks.is_empty() {
                String::new()
            } else {
                format!("\n  Pre-screening risk flags: {}", s.risks.join("; "))
            };
            format!(
                "[{}] {}{year} [{}] {genres} | TMDB rating {} ({} votes){}{library}{risks}\n  {}\n  Research:\n{}",
                c.canonical_id,
                c.title,
                c.media_type.as_str(),
                to_fixed(c.vote_average, 1),
                number_to_string(c.vote_count),
                format_candidate_details(c, true),
                slice_utf16(&collapse_whitespace(&c.overview), 400),
                research
                    .get(&c.canonical_id)
                    .map_or("undefined", String::as_str)
            )
        })
        .collect();
    format!(
        "You are choosing at most ONE title to add to one person's media watchlist today. Your job is precision, not activity: a skipped day costs nothing; a bad pick erodes trust in every future recommendation.

THE USER'S TASTE EVIDENCE (watch history is ground truth; explicit good/not-for-me feedback is direct preference evidence):
{history_digest}

FINALISTS (pre-screened for taste fit):
{}

PROCESS:
1. Evaluate the supplied research for critical reception, audience sentiment, whether the title holds up, and material caveats.
2. Compare against the user's actual watch history — justify against what they demonstrably watch and finish, not generic acclaim.
3. Decide: select exactly one, or no_add if the evidence is weak for all finalists. Prefer the smaller, higher-confidence commitment when two options are similarly good. no_add is a respectable outcome, not a failure.

Return the structured decision. Candidate ids must come from the list above. The backup is your second choice, used only if the first turns out to already be on the watchlist, so give it its own honest why_for_user and notification copy (never reuse the primary's). why_for_user should reference their actual watching patterns. Keep notification messages concise and concrete. For no_add, set selected and backup to null and explain in no_add_reason.",
        blocks.join("\n\n")
    )
}
