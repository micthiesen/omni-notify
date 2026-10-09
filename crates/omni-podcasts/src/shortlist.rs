//! Cheap-model scoring of eligible episodes (`shortlist.ts`). Ordering is
//! computed in code from the scores, never from model prose.

use std::collections::HashMap;

use omni_ai::ModelRole;
use schemars::JsonSchema;
use serde::Deserialize;

use crate::discovery::collapse_whitespace;
use crate::js::{to_date_stamp, to_fixed};
use crate::log_file::{self, LogFile, code_block};
use crate::models::{Models, Refine, in_range};
use crate::pipeline::PipelineError;
use crate::types::EpisodeCandidate;

const LOG: &str = "PodcastRecsTask";
pub const FINALIST_COUNT: usize = 4;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct EpisodeScore {
    pub candidate_id: String,
    pub taste_match: f64,
    pub novelty: f64,
    pub confidence: f64,
    pub risks: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ShortlistScoresOutput {
    pub scores: Vec<EpisodeScore>,
}

impl Refine for ShortlistScoresOutput {
    fn refine(&self) -> Result<(), String> {
        self.scores.iter().try_for_each(|s| {
            in_range(s.taste_match, 0.0, 100.0, "taste_match")?;
            in_range(s.novelty, 0.0, 100.0, "novelty")?;
            in_range(s.confidence, 0.0, 1.0, "confidence")
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScoredEpisode {
    pub candidate: EpisodeCandidate,
    pub taste_match: f64,
    pub novelty: f64,
    pub confidence: f64,
    pub risks: Vec<String>,
    pub composite: f64,
}

/// Taste dominates, novelty breaks ties, confidence scales.
pub fn compute_composite(taste_match: f64, novelty: f64, confidence: f64) -> f64 {
    let base = 0.7 * taste_match + 0.3 * novelty;
    base * (0.5 + 0.5 * confidence)
}

pub async fn shortlist_episodes(
    models: &Models,
    candidates: &[EpisodeCandidate],
    taste_digest: &str,
    log_file: Option<&LogFile>,
    finalist_count: usize,
) -> Result<Vec<ScoredEpisode>, PipelineError> {
    let model_id = models.model_id(ModelRole::RecsShortlist);
    // Deterministic ordering reduces position bias.
    let mut ordered: Vec<&EpisodeCandidate> = candidates.iter().collect();
    ordered.sort_by(|a, b| omni_core::js::locale_compare(&a.episode_id, &b.episode_id));
    let prompt = build_prompt(&ordered, taste_digest);
    log_file::section(
        log_file,
        &format!("Podcast Shortlist Prompt ({model_id})"),
        &code_block(&prompt, None),
    )
    .await?;
    tracing::info!(target: LOG, "Scoring {} episodes ({model_id})", ordered.len());

    let generated = models
        .object::<ShortlistScoresOutput>(ModelRole::RecsShortlist, "shortlist", prompt)
        .await
        .map_err(PipelineError::model("shortlist episodes"))?;
    tracing::info!(
        target: LOG,
        "Shortlist token usage: {} prompt, {} completion",
        generated.usage.input_tokens,
        generated.usage.output_tokens
    );

    let by_id: HashMap<&str, &EpisodeCandidate> = candidates
        .iter()
        .map(|c| (c.episode_id.as_str(), c))
        .collect();
    let mut scored = Vec::new();
    for score in generated.output.scores {
        let Some(candidate) = by_id.get(score.candidate_id.as_str()) else {
            tracing::warn!(target: LOG, "Shortlist returned unknown candidate_id: {}", score.candidate_id);
            continue;
        };
        scored.push(ScoredEpisode {
            candidate: (*candidate).clone(),
            taste_match: score.taste_match,
            novelty: score.novelty,
            confidence: score.confidence,
            composite: compute_composite(score.taste_match, score.novelty, score.confidence),
            risks: score.risks,
        });
    }
    scored.sort_by(|a, b| {
        b.composite
            .partial_cmp(&a.composite)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scored.truncate(finalist_count);

    let lines = scored
        .iter()
        .map(|s| {
            format!(
                "{} {} — {} (taste={} novelty={} conf={})",
                to_fixed(s.composite, 1),
                s.candidate.show_title,
                s.candidate.episode_title,
                omni_core::js::number_to_string(s.taste_match),
                omni_core::js::number_to_string(s.novelty),
                omni_core::js::number_to_string(s.confidence)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    log_file::section(
        log_file,
        "Podcast Shortlist Result",
        &code_block(&lines, None),
    )
    .await?;
    Ok(scored)
}

fn build_prompt(candidates: &[&EpisodeCandidate], taste_digest: &str) -> String {
    let lines: Vec<String> = candidates
        .iter()
        .map(|c| {
            let duration = c.duration_minutes.filter(|d| *d != 0).map(|d| format!(" | {d} min")).unwrap_or_default();
            let genres = if c.show_genres.is_empty() {
                "unknown genres".to_owned()
            } else {
                c.show_genres.join("/")
            };
            let description = omni_core::js::utf16_slice(&collapse_whitespace(&c.description), 0, 220).into_owned();
            format!(
                "[{}] {} — {} | {genres} | released {}{duration} | surfaced via: {}\n  {description}",
                c.episode_id,
                c.show_title,
                c.episode_title,
                to_date_stamp(c.published_at),
                c.discovered_via
            )
        })
        .collect();
    format!(
        "You are a conservative scorer of podcast EPISODES for one specific listener. You are NOT choosing a winner — score each candidate from the supplied facts only. Do not invent metadata.

THE LISTENER (subscribed shows are ground truth; explicit feedback is direct preference evidence):
{taste_digest}

SCORING DIMENSIONS (0-100 each):
- taste_match: fit with the tastes evident in the subscribed shows and profile (long-form intellectual conversation over vibes; not generic popularity).
- novelty: rewards shows/hosts/perspectives adjacent-but-new relative to what they already follow; penalize both near-clones of subscribed shows and total non-sequiturs.

Also return confidence (0-1) in your own scoring, and risk flags (e.g. \"possibly promotional\", \"host known for outrage content\", \"episode topic may be stale news\").

Score every candidate. Return JSON only.

CANDIDATES:
{}",
        lines.join("\n")
    )
}
