//! Cheap-model scoring of every eligible candidate.

use std::collections::HashMap;

use omni_ai::{Ai, CostTag, GenerateRequest, LanguageModel, ModelRole};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::error::IntegrationError;
use crate::js::{code_block, collapse_whitespace, number, slice_utf16, to_fixed};
use crate::run_log::{self, RunLogFile};
use crate::types::{Candidate, MediaType};

pub const FINALIST_COUNT: usize = 5;
const LOG: &str = "Main:RecsTask";

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct ScoreOutput {
    pub scores: Vec<CandidateScore>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct CandidateScore {
    pub candidate_id: String,
    #[schemars(range(min = 0, max = 100))]
    pub taste_match: f64,
    #[schemars(range(min = 0, max = 100))]
    pub novelty: f64,
    #[schemars(range(min = 0, max = 100))]
    pub effort_fit: f64,
    #[schemars(range(min = 0, max = 1))]
    pub confidence: f64,
    pub risks: Vec<String>,
}

impl ScoreOutput {
    /// The zod bounds the AI SDK enforced on parse.
    fn validate(&self) -> Result<(), String> {
        for score in &self.scores {
            let in_range = |v: f64, max: f64| (0.0..=max).contains(&v);
            if !in_range(score.taste_match, 100.0)
                || !in_range(score.novelty, 100.0)
                || !in_range(score.effort_fit, 100.0)
                || !in_range(score.confidence, 1.0)
            {
                return Err(format!(
                    "No object generated: score for {} is out of range",
                    score.candidate_id
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScoredCandidate {
    pub candidate: Candidate,
    pub taste_match: f64,
    pub novelty: f64,
    pub effort_fit: f64,
    pub confidence: f64,
    pub risks: Vec<String>,
    pub composite: f64,
}

/// Ranking score computed in code: weighted dimensions, shrunk toward the
/// middle (not to zero) at low confidence.
pub fn compute_composite(taste_match: f64, novelty: f64, effort_fit: f64, confidence: f64) -> f64 {
    let base = 0.55 * taste_match + 0.25 * novelty + 0.2 * effort_fit;
    base * (0.5 + 0.5 * confidence)
}

/// Scores all candidates with the shortlist model and keeps the top `finalist_count`.
pub async fn shortlist_candidates(
    ai: &Ai,
    model: &dyn LanguageModel,
    candidates: &[Candidate],
    history_digest: &str,
    log_file: Option<&RunLogFile>,
    finalist_count: usize,
) -> Result<Vec<ScoredCandidate>, IntegrationError> {
    let model_id = model.id().to_string();
    let mut ordered: Vec<&Candidate> = candidates.iter().collect();
    ordered.sort_by_key(|c| c.tmdb_id);
    let prompt = build_prompt(&ordered, history_digest);

    if let Some(log) = log_file {
        log.section(
            &format!("Shortlist Prompt ({model_id})"),
            &code_block(&prompt, None),
        )
        .await?;
        tracing::info!(target: LOG, "Scoring {} candidates ({model_id})", ordered.len());
    }

    let operation = "generate recommendation shortlist";
    let (output, usage) = ai
        .generate_object::<ScoreOutput>(
            model,
            GenerateRequest::prompt(prompt),
            CostTag::for_role(ModelRole::RecsShortlist),
        )
        .await
        .map_err(|e| IntegrationError::from_error(operation, &e))?;
    output
        .validate()
        .map_err(|e| IntegrationError::new(operation, e))?;
    tracing::info!(
        target: LOG,
        "Shortlist token usage: {} prompt, {} completion",
        usage.input_tokens,
        usage.output_tokens
    );

    let by_id: HashMap<&str, &Candidate> = candidates
        .iter()
        .map(|c| (c.canonical_id.as_str(), c))
        .collect();
    let mut scored: Vec<ScoredCandidate> = Vec::new();
    for score in output.scores {
        let Some(candidate) = by_id.get(score.candidate_id.as_str()) else {
            tracing::warn!(
                target: LOG,
                "Shortlist returned unknown candidate_id: {}",
                score.candidate_id
            );
            continue;
        };
        scored.push(ScoredCandidate {
            candidate: (*candidate).clone(),
            composite: compute_composite(
                score.taste_match,
                score.novelty,
                score.effort_fit,
                score.confidence,
            ),
            taste_match: score.taste_match,
            novelty: score.novelty,
            effort_fit: score.effort_fit,
            confidence: score.confidence,
            risks: score.risks,
        });
    }
    scored.sort_by(|a, b| b.composite.total_cmp(&a.composite));
    scored.truncate(finalist_count);

    let summary = scored
        .iter()
        .map(|s| {
            format!(
                "{} {} (taste={} novelty={} effort={} conf={})",
                to_fixed(s.composite, 1),
                s.candidate.title,
                number(s.taste_match),
                number(s.novelty),
                number(s.effort_fit),
                number(s.confidence)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    run_log::section(log_file, "Shortlist Result", &code_block(&summary, None)).await?;
    Ok(scored)
}

fn year_suffix(year: Option<i64>) -> String {
    year.filter(|y| *y != 0)
        .map(|y| format!(" ({y})"))
        .unwrap_or_default()
}

fn build_prompt(candidates: &[&Candidate], history_digest: &str) -> String {
    let lines: Vec<String> = candidates
        .iter()
        .map(|c| {
            let genres = if c.genres.is_empty() {
                "unknown genres".to_owned()
            } else {
                c.genres.join("/")
            };
            let library = if c.in_library { " | IN LOCAL LIBRARY" } else { "" };
            let overview = slice_utf16(&collapse_whitespace(&c.overview), 180);
            format!(
                "[{}] {}{} [{}] {genres} | rating {} ({} votes) | source={}{library}{}\n  {overview}",
                c.canonical_id,
                c.title,
                year_suffix(c.year),
                c.media_type.as_str(),
                to_fixed(c.vote_average, 1),
                number(c.vote_count),
                c.source.as_str(),
                format_candidate_details(c, false)
            )
        })
        .collect();
    format!(
        "You are a conservative recommendation scorer for one person's media watchlist. You are NOT choosing a winner — you are scoring each candidate from the supplied facts only. Do not invent metadata or use knowledge that contradicts the provided facts.

THE USER'S TASTE EVIDENCE (watch history is ground truth; explicit good/not-for-me feedback is direct preference evidence):
{history_digest}

SCORING DIMENSIONS (0-100 each):
- taste_match: how well this fits the tastes evident in the watch history (not generic acclaim).
- novelty: rewards fresh-but-plausible territory; penalize both carbon copies of recent watches and wild leaps with no anchor in the history.
- effort_fit: how likely they are to actually start it soon. Movies and limited series score higher than huge multi-season commitments unless the history shows they finish long shows.

Also return confidence (0-1) in your own scoring for that candidate, and any risk flags (e.g. \"long commitment\", \"divisive reception\", \"very similar to X they just watched\").

Score every candidate. Return JSON only.

CANDIDATES:
{}",
        lines.join("\n")
    )
}

fn truthy(value: Option<f64>) -> Option<f64> {
    value.filter(|v| *v != 0.0 && !v.is_nan())
}

/// Viewing commitment and structured context for model prompts.
pub fn format_candidate_details(candidate: &Candidate, include_creative_context: bool) -> String {
    let mut details: Vec<String> = Vec::new();
    if let Some(runtime) = truthy(candidate.runtime_minutes) {
        details.push(match candidate.media_type {
            MediaType::Tv => format!("{} min/episode", number(runtime)),
            MediaType::Movie => format!("{} min", number(runtime)),
        });
    }
    if let Some(seasons) = truthy(candidate.season_count) {
        details.push(format!("{} seasons", number(seasons)));
    }
    if let Some(episodes) = truthy(candidate.episode_count) {
        details.push(format!("{} episodes", number(episodes)));
    }
    if let Some(status) = candidate.series_status.as_deref().filter(|s| !s.is_empty()) {
        details.push(status.to_owned());
    }
    if let Some(certification) = candidate.certification.as_deref().filter(|s| !s.is_empty()) {
        details.push(certification.to_owned());
    }
    if let Some(language) = candidate
        .original_language
        .as_deref()
        .filter(|l| !l.is_empty() && *l != "en")
    {
        details.push(format!("language={language}"));
    }
    if let Some(countries) = candidate
        .origin_countries
        .as_ref()
        .filter(|c| !c.is_empty())
    {
        details.push(format!("origin={}", countries.join(",")));
    }
    if include_creative_context {
        if let Some(creators) = candidate.creators.as_ref().filter(|c| !c.is_empty()) {
            details.push(format!("creator={}", creators.join(", ")));
        }
        if let Some(cast) = candidate.cast.as_ref().filter(|c| !c.is_empty()) {
            let top: Vec<&str> = cast.iter().take(4).map(String::as_str).collect();
            details.push(format!("cast={}", top.join(", ")));
        }
        if let Some(keywords) = candidate.keywords.as_ref().filter(|k| !k.is_empty()) {
            let top: Vec<&str> = keywords.iter().take(6).map(String::as_str).collect();
            details.push(format!("themes={}", top.join(", ")));
        }
    }
    if details.is_empty() {
        String::new()
    } else {
        format!(" | {}", details.join(" | "))
    }
}
