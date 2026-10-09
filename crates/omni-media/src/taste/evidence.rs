//! Deterministic taste evidence (`src/recommendations/taste/evidence.ts`).
//!
//! Evidence ids hash `JSON.stringify` output with JS key order and number
//! formatting; they must match the ids TS wrote, or re-polling the same
//! state would insert duplicates.

use omni_api::media::RecommendationStatus;
use omni_core::digest::digest;
use omni_core::js::json_stringify;
use omni_store::cbor::JsValue;
use serde_json::{Map, Value};

use super::types::{CanonicalWatchObservation, TasteEvidenceData, TasteEvidenceKind};
use crate::js::{number, to_fixed};
use crate::persistence::RecommendationData;

fn put<T: serde::Serialize>(map: &mut Map<String, Value>, key: &str, value: Option<&T>) {
    if let Some(value) = value.and_then(|v| serde_json::to_value(v).ok()) {
        map.insert(key.to_owned(), value);
    }
}

#[allow(clippy::cast_precision_loss)]
fn int(value: i64) -> String {
    number(value as f64)
}

pub fn derive_watch_evidence(observations: &[CanonicalWatchObservation]) -> Vec<TasteEvidenceData> {
    observations
        .iter()
        .map(|observation| {
            let item = &observation.item;
            let metadata_json = match &observation.metadata {
                Some(metadata) => serde_json::to_value(metadata)
                    .map(|v| json_stringify(&v))
                    .unwrap_or_else(|_| "{}".to_owned()),
                None => "{}".to_owned(),
            };
            let identity = [
                observation.canonical_id.clone(),
                int(item.viewed_at),
                int(item.view_count),
                item.completion
                    .map_or_else(|| "unknown".to_owned(), |c| to_fixed(c, 3)),
                metadata_json,
            ]
            .join(":");
            let metadata = observation.metadata.clone().unwrap_or_default();
            let has_metadata = observation.metadata.is_some();
            TasteEvidenceData {
                evidence_id: format!("plex:{}", digest(&identity)),
                kind: TasteEvidenceKind::PlexWatch,
                canonical_id: observation.canonical_id.clone(),
                title: item.item.title.clone(),
                year: item.item.year,
                media_type: item.item.media_type,
                observed_at: item.viewed_at,
                view_count: Some(item.view_count),
                completion: item.completion,
                genres: has_metadata.then_some(metadata.genres),
                runtime_minutes: metadata.runtime_minutes,
                season_count: metadata.season_count,
                episode_count: metadata.episode_count,
                series_status: metadata.series_status,
                original_language: metadata.original_language,
                origin_countries: has_metadata.then_some(metadata.origin_countries),
                creators: has_metadata.then_some(metadata.creators),
                cast: has_metadata.then_some(metadata.cast),
                keywords: has_metadata.then_some(metadata.keywords),
                certification: metadata.certification,
                ..TasteEvidenceData::default()
            }
        })
        .collect()
}

/// The recommendation fields every recommendation evidence row carries, in
/// TS object-literal order (`recommendationFields`).
fn recommendation_fields(rec: &RecommendationData) -> Map<String, Value> {
    let mut fields = Map::new();
    put(&mut fields, "canonicalId", Some(&rec.canonical_id));
    put(&mut fields, "title", Some(&rec.title));
    put(&mut fields, "year", rec.year.as_ref());
    put(&mut fields, "mediaType", Some(&rec.media_type));
    put(
        &mut fields,
        "recommendationId",
        Some(&rec.recommendation_id),
    );
    put(&mut fields, "recommendedAt", Some(&rec.recommended_at));
    put(&mut fields, "startedAt", rec.started_at.as_ref());
    put(&mut fields, "source", rec.source.as_ref());
    put(&mut fields, "genres", rec.genres.as_ref());
    put(&mut fields, "runtimeMinutes", rec.runtime_minutes.as_ref());
    put(&mut fields, "seasonCount", rec.season_count.as_ref());
    put(&mut fields, "episodeCount", rec.episode_count.as_ref());
    put(&mut fields, "seriesStatus", rec.series_status.as_ref());
    put(
        &mut fields,
        "originalLanguage",
        rec.original_language.as_ref(),
    );
    put(
        &mut fields,
        "originCountries",
        rec.origin_countries.as_ref(),
    );
    put(&mut fields, "creators", rec.creators.as_ref());
    put(&mut fields, "cast", rec.cast.as_ref());
    put(&mut fields, "keywords", rec.keywords.as_ref());
    put(&mut fields, "certification", rec.certification.as_ref());
    fields
}

fn base_evidence(
    rec: &RecommendationData,
    kind: TasteEvidenceKind,
    observed_at: i64,
) -> TasteEvidenceData {
    TasteEvidenceData {
        kind,
        canonical_id: rec.canonical_id.clone(),
        title: rec.title.clone(),
        year: rec.year,
        media_type: rec.media_type,
        observed_at,
        recommendation_id: Some(rec.recommendation_id.clone()),
        recommended_at: Some(rec.recommended_at),
        started_at: rec.started_at,
        source: rec.source.map(|s| s.as_str().to_owned()),
        genres: rec.genres.clone(),
        runtime_minutes: rec.runtime_minutes,
        season_count: rec.season_count,
        episode_count: rec.episode_count,
        series_status: rec.series_status.clone(),
        original_language: rec.original_language.clone(),
        origin_countries: rec.origin_countries.clone(),
        creators: rec.creators.clone(),
        cast: rec.cast.clone(),
        keywords: rec.keywords.clone(),
        certification: rec.certification.clone(),
        ..TasteEvidenceData::default()
    }
}

/// One outcome row per recommendation state, plus a feedback row when it
/// carries a rating or a note.
pub fn derive_recommendation_evidence(
    recommendations: &[RecommendationData],
) -> Vec<TasteEvidenceData> {
    let mut evidence = Vec::new();
    for rec in recommendations {
        let fields = json_stringify(&Value::Object(recommendation_fields(rec)));
        let observed_at = rec
            .resolved_at
            .or(rec.notified_at)
            .unwrap_or(rec.recommended_at);
        let status: RecommendationStatus = rec.status();
        let identity = [
            rec.recommendation_id.clone(),
            "outcome".to_owned(),
            status.as_str().to_owned(),
            int(observed_at),
            fields.clone(),
        ]
        .join(":");
        evidence.push(TasteEvidenceData {
            evidence_id: format!("recommendation:{}", digest(&identity)),
            recommendation_status: Some(status),
            ..base_evidence(rec, TasteEvidenceKind::RecommendationOutcome, observed_at)
        });

        let note = rec.feedback_note.as_deref().filter(|n| !n.is_empty());
        if rec.feedback.is_some() || note.is_some() {
            let observed_at = rec.feedback_at.unwrap_or(rec.recommended_at);
            let identity = [
                rec.recommendation_id.clone(),
                "feedback".to_owned(),
                rec.feedback.map_or("none", |f| f.as_str()).to_owned(),
                rec.feedback_note.clone().unwrap_or_default(),
                int(observed_at),
                fields,
            ]
            .join(":");
            evidence.push(TasteEvidenceData {
                evidence_id: format!("recommendation:{}", digest(&identity)),
                feedback: rec.feedback,
                note: rec.feedback_note.clone(),
                ..base_evidence(rec, TasteEvidenceKind::ExplicitFeedback, observed_at)
            });
        }
    }
    evidence
}

/// `fingerprintEvidence`: sorted ids, sorted keys, `undefined` dropped.
pub fn fingerprint_evidence(evidence: &[TasteEvidenceData]) -> String {
    let cleaned: Vec<TasteEvidenceData> = evidence
        .iter()
        .map(|item| {
            let mut item = item.clone();
            item.extra
                .retain(|_, value| !matches!(value, JsValue::Undefined));
            item
        })
        .collect();
    omni_core::digest::fingerprint_evidence(&cleaned, |item| item.evidence_id.as_str())
        .unwrap_or_default()
}
