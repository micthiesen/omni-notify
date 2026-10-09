//! Tier-1 gate for guest appearances: default-include,
//! drop only off-taste, trivial or namesake episodes, keep up to `max`.

use std::collections::{HashMap, HashSet};

use omni_ai::ModelRole;
use schemars::JsonSchema;
use serde::Deserialize;

use crate::discovery::collapse_whitespace;
use crate::js::to_date_stamp;
use crate::log_file::{self, LogFile, code_block};
use crate::models::{Models, Refine, in_range};
use crate::pipeline::PipelineError;
use crate::selection::{NotificationCopy, PodcastSelectionPick};
use crate::types::EpisodeCandidate;

const LOG: &str = "PodcastRecsTask";

#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
pub struct GuestDecision {
    pub candidate_id: String,
    pub include: bool,
    pub reason: String,
    pub why_for_user: Option<String>,
    pub caveats: Vec<String>,
    pub confidence: f64,
    pub notification: Option<NotificationCopy>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GuestDecisions {
    pub decisions: Vec<GuestDecision>,
}

impl Refine for GuestDecisions {
    fn refine(&self) -> Result<(), String> {
        self.decisions
            .iter()
            .try_for_each(|d| in_range(d.confidence, 0.0, 1.0, "confidence"))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct GuestPick {
    pub candidate: EpisodeCandidate,
    pub pick: PodcastSelectionPick,
}

/// Includes only, strongest confidence first, de-duplicated by id (a repeat
/// would double-commit), unknown ids and copy-less includes dropped, capped.
pub fn apply_guest_decisions(
    decisions: &[GuestDecision],
    candidates_by_id: &HashMap<String, EpisodeCandidate>,
    max: usize,
) -> Vec<GuestPick> {
    let mut included: Vec<&GuestDecision> = decisions.iter().filter(|d| d.include).collect();
    included.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut picks = Vec::new();
    let mut seen = HashSet::new();
    for decision in included {
        if picks.len() >= max {
            break;
        }
        if !seen.insert(decision.candidate_id.clone()) {
            continue;
        }
        let Some(candidate) = candidates_by_id.get(&decision.candidate_id) else {
            tracing::warn!(target: LOG, "Guest gate returned unknown candidate_id: {}", decision.candidate_id);
            continue;
        };
        let (Some(why), Some(notification)) = (
            decision.why_for_user.as_ref().filter(|w| !w.is_empty()),
            decision.notification.as_ref(),
        ) else {
            tracing::warn!(
                target: LOG,
                "Guest gate included {} — {} without copy; skipping",
                candidate.show_title,
                candidate.episode_title
            );
            continue;
        };
        picks.push(GuestPick {
            candidate: candidate.clone(),
            pick: PodcastSelectionPick {
                candidate_id: decision.candidate_id.clone(),
                why_for_user: why.clone(),
                caveats: decision.caveats.clone(),
                confidence: decision.confidence,
                notification: notification.clone(),
            },
        });
    }
    picks
}

pub async fn select_guest_appearances(
    models: &Models,
    candidates: &[EpisodeCandidate],
    taste_digest: &str,
    log_file: Option<&LogFile>,
    max: usize,
) -> Result<Vec<GuestPick>, PipelineError> {
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    let model_id = models.model_id(ModelRole::RecsSelection);
    let prompt = build_prompt(candidates, taste_digest, max);
    log_file::section(
        log_file,
        &format!("Guest Gate Prompt ({model_id})"),
        &code_block(&prompt, None),
    )
    .await?;
    tracing::info!(target: LOG, "Gating {} guest candidate(s) ({model_id})", candidates.len());
    let generated = models
        .object::<GuestDecisions>(ModelRole::RecsSelection, "select-guest-appearances", prompt)
        .await
        .map_err(PipelineError::model("select guest appearances"))?;
    tracing::info!(
        target: LOG,
        "Guest gate token usage: {} prompt, {} completion",
        generated.usage.input_tokens,
        generated.usage.output_tokens
    );
    let by_id: HashMap<String, EpisodeCandidate> = candidates
        .iter()
        .map(|c| (c.episode_id.clone(), c.clone()))
        .collect();
    let picks = apply_guest_decisions(&generated.output.decisions, &by_id, max);
    let listing = picks
        .iter()
        .map(|p| {
            format!(
                "INCLUDE {} — {}",
                p.candidate.show_title, p.candidate.episode_title
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    log_file::section(
        log_file,
        "Guest Gate",
        if listing.is_empty() {
            "none included"
        } else {
            &listing
        },
    )
    .await?;
    Ok(picks)
}

fn build_prompt(candidates: &[EpisodeCandidate], taste_digest: &str, max: usize) -> String {
    let blocks: Vec<String> = candidates
        .iter()
        .map(|c| {
            let duration = c
                .duration_minutes
                .filter(|d| *d != 0)
                .map(|d| format!(" | {d} min"))
                .unwrap_or_default();
            let description =
                omni_core::js::utf16_slice(&collapse_whitespace(&c.description), 0, 300)
                    .into_owned();
            format!(
                "[{}] {} — {} | featuring: {} | released {}{duration}\n  {description}",
                c.episode_id,
                c.show_title,
                c.episode_title,
                c.matched_voices.clone().unwrap_or_default().join(", "),
                to_date_stamp(c.published_at)
            )
        })
        .collect();
    format!(
        "You are gating podcast episodes where a voice the listener FOLLOWS appears as a guest. Following the person is a strong positive signal, so DEFAULT TO INCLUDE — only exclude an episode if it is clearly off-taste (grift/guru/outrage/rage-farming/promotional or sponsored), clearly trivial (a brief mention, a rerun, not a real substantive appearance), or clearly a NAMESAKE (a different person who happens to share the followed voice's name — e.g. a pastor named Sean Carroll is not the physicist). Exclude namesakes.

THE LISTENER:
{taste_digest}

Decide include (default true) or exclude for each candidate. Include at most {max}; if more qualify, keep the strongest. For each INCLUDED episode, write why_for_user (reference the followed guest and the listener's taste) and a notification: a title prefixed with a topic emoji and naming the show + guest, plus a concrete 2-3 sentence message. For EXCLUDED episodes set why_for_user and notification to null and give a short reason.

CANDIDATES:
{}

Return JSON only.",
        blocks.join("\n")
    )
}
