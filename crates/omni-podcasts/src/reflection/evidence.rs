//! Evidence derivation with deterministic ids.
//! Ids are `digest()`s of JS-formatted observation strings, so the same
//! observation always maps to the same append-only row.

use omni_core::digest::digest;
use omni_core::js::{json_stringify, number_to_string, to_fixed};
use omni_store::cbor::Extra;
use serde_json::{Map, Value};

use super::types::{PodcastTasteEvidenceData, PodcastTasteEvidenceKind};
use crate::account::ListenedEpisode;
use crate::persistence::{PodcastFeedback, PodcastRecommendationData};

/// Trimmed, lowercased show title.
pub fn normalize_show_key(show_title: &str) -> String {
    show_title.trim().to_lowercase()
}

/// Order-independent digest of the evidence set.
pub fn fingerprint_evidence(
    evidence: &[PodcastTasteEvidenceData],
) -> Result<String, serde_json::Error> {
    omni_core::digest::fingerprint_evidence(evidence, |e| e.evidence_id.as_str())
}

fn ms(value: i64) -> String {
    #[allow(clippy::cast_precision_loss)]
    number_to_string(value as f64)
}

pub fn derive_listen_evidence(listened: &[ListenedEpisode]) -> Vec<PodcastTasteEvidenceData> {
    listened
        .iter()
        .map(|item| {
            let identity = [
                normalize_show_key(&item.show_title),
                item.episode_guid
                    .clone()
                    .unwrap_or_else(|| item.episode_title.clone()),
                ms(item.listened_at),
                item.completion
                    .map_or_else(|| "unknown".to_owned(), |c| to_fixed(c, 3)),
                (item.starred == Some(true)).to_string(),
            ]
            .join(":");
            PodcastTasteEvidenceData {
                evidence_id: format!("listen:{}", digest(&identity)),
                kind: PodcastTasteEvidenceKind::Listen,
                show_key: normalize_show_key(&item.show_title),
                show_title: item.show_title.clone(),
                episode_title: Some(item.episode_title.clone()),
                recommendation_id: None,
                discovered_via: None,
                matched_voices: None,
                duration_minutes: None,
                observed_at: item.listened_at,
                completion: item.completion,
                starred: item.starred,
                recommendation_status: None,
                feedback: None,
                note: None,
                extra: Extra::default(),
            }
        })
        .collect()
}

/// `JSON.stringify(recommendationFields(rec))` with undefined fields dropped.
fn fields_json(rec: &PodcastRecommendationData) -> String {
    let mut fields = Map::new();
    fields.insert(
        "showKey".into(),
        Value::String(normalize_show_key(&rec.show_title)),
    );
    fields.insert("showTitle".into(), Value::String(rec.show_title.clone()));
    fields.insert(
        "episodeTitle".into(),
        Value::String(rec.episode_title.clone()),
    );
    fields.insert(
        "recommendationId".into(),
        Value::String(rec.recommendation_id.clone()),
    );
    if let Some(via) = &rec.discovered_via {
        fields.insert("discoveredVia".into(), Value::String(via.clone()));
    }
    if let Some(voices) = &rec.matched_voices {
        fields.insert(
            "matchedVoices".into(),
            Value::Array(voices.iter().cloned().map(Value::String).collect()),
        );
    }
    if let Some(duration) = rec.duration_minutes {
        fields.insert("durationMinutes".into(), Value::from(duration));
    }
    json_stringify(&Value::Object(fields))
}

fn base(
    rec: &PodcastRecommendationData,
    evidence_id: String,
    kind: PodcastTasteEvidenceKind,
    observed_at: i64,
) -> PodcastTasteEvidenceData {
    PodcastTasteEvidenceData {
        evidence_id,
        kind,
        show_key: normalize_show_key(&rec.show_title),
        show_title: rec.show_title.clone(),
        episode_title: Some(rec.episode_title.clone()),
        recommendation_id: Some(rec.recommendation_id.clone()),
        discovered_via: rec.discovered_via.clone(),
        matched_voices: rec.matched_voices.clone(),
        duration_minutes: rec.duration_minutes,
        observed_at,
        completion: None,
        starred: None,
        recommendation_status: None,
        feedback: None,
        note: None,
        extra: Extra::default(),
    }
}

/// One outcome row per recommendation, plus a feedback row when it carries
/// feedback or a note.
pub fn derive_recommendation_evidence(
    recommendations: &[PodcastRecommendationData],
) -> Vec<PodcastTasteEvidenceData> {
    let mut evidence = Vec::new();
    for rec in recommendations {
        let fields = fields_json(rec);
        let observed_at = rec
            .resolved_at
            .or(rec.notified_at)
            .unwrap_or(rec.recommended_at);
        let outcome_id = [
            rec.recommendation_id.clone(),
            "outcome".to_owned(),
            rec.status.as_str().to_owned(),
            ms(observed_at),
            fields.clone(),
        ]
        .join(":");
        let mut outcome = base(
            rec,
            format!("recommendation:{}", digest(&outcome_id)),
            PodcastTasteEvidenceKind::RecommendationOutcome,
            observed_at,
        );
        outcome.recommendation_status = Some(rec.status);
        evidence.push(outcome);

        let note = rec.feedback_note.as_deref().filter(|n| !n.is_empty());
        if rec.feedback.is_some() || note.is_some() {
            let feedback_at = rec.feedback_at.unwrap_or(rec.recommended_at);
            let feedback_name = match rec.feedback {
                Some(PodcastFeedback::GoodPick) => "good_pick",
                Some(PodcastFeedback::NotForMe) => "not_for_me",
                None => "none",
            };
            let feedback_id = [
                rec.recommendation_id.clone(),
                "feedback".to_owned(),
                feedback_name.to_owned(),
                rec.feedback_note.clone().unwrap_or_default(),
                ms(feedback_at),
                fields,
            ]
            .join(":");
            let mut row = base(
                rec,
                format!("recommendation:{}", digest(&feedback_id)),
                PodcastTasteEvidenceKind::ExplicitFeedback,
                feedback_at,
            );
            row.feedback = rec.feedback;
            row.note = rec.feedback_note.clone();
            evidence.push(row);
        }
    }
    evidence
}
