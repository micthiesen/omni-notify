//! PressPods DTOs: `/api/press-pods/*` and the `/pods/episodes` reply.

use std::fmt;
use std::marker::PhantomData;

use serde::de::{MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A JSON object kept in its wire order. Stored cost detail maps are in
/// insertion order and the UI lists them that way, so a sorted map would
/// reorder the breakdown.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OrderedMap<V>(pub Vec<(String, V)>);

impl<V> FromIterator<(String, V)> for OrderedMap<V> {
    fn from_iter<I: IntoIterator<Item = (String, V)>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl<V: Serialize> Serialize for OrderedMap<V> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (key, value) in &self.0 {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}

impl<'de, V: Deserialize<'de>> Deserialize<'de> for OrderedMap<V> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct OrderedVisitor<V>(PhantomData<V>);
        impl<'de, V: Deserialize<'de>> Visitor<'de> for OrderedVisitor<V> {
            type Value = OrderedMap<V>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
                let mut entries = Vec::with_capacity(access.size_hint().unwrap_or(0));
                while let Some(entry) = access.next_entry::<String, V>()? {
                    entries.push(entry);
                }
                Ok(OrderedMap(entries))
            }
        }
        deserializer.deserialize_map(OrderedVisitor(PhantomData))
    }
}

/// `PressPodsRetrieverAttempt`: a rated success or a failure.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PressPodsRetrieverAttempt {
    Success {
        name: String,
        /// Always `true`.
        success: bool,
        #[serde(rename = "contentRating")]
        content_rating: f64,
        #[serde(rename = "textChars")]
        text_chars: i64,
    },
    Failure {
        name: String,
        /// Always `false`.
        success: bool,
        error: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PressPodsChapter {
    pub start_time_seconds: f64,
    pub title: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PressPodsChunkStat {
    pub index: i64,
    pub section_index: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section_title: Option<String>,
    pub text: String,
    pub char_count: i64,
    pub duration_seconds: f64,
    pub start_time_seconds: f64,
    pub sec_per_char: f64,
    pub attempts: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub word_ratio: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_words: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resplit: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resplit_depth: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PressPodsTokenCounts {
    pub input: f64,
    pub output: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PressPodsCosts {
    pub llm_cents: f64,
    pub tts_cents: f64,
    pub detail_cents: OrderedMap<f64>,
    pub detail_tokens: OrderedMap<PressPodsTokenCounts>,
    pub detail_chars: OrderedMap<f64>,
}

/// One episode in the list payload.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PressPodsEpisode {
    pub episode_id: String,
    pub title: String,
    pub author: Option<String>,
    pub publication: Option<String>,
    pub domain: Option<String>,
    pub article_url: String,
    pub lead_image_url: Option<String>,
    pub excerpt: Option<String>,
    pub voice_name: Option<String>,
    pub synthesized_seconds: Option<f64>,
    pub chapters: Option<Vec<PressPodsChapter>>,
    pub audio_url: String,
    pub duration_seconds: Option<f64>,
    pub file_bytes: i64,
    pub retriever_name: Option<String>,
    pub retriever_seconds: Option<f64>,
    pub retriever_attempts: Option<Vec<PressPodsRetrieverAttempt>>,
    pub cost_cents: Option<f64>,
    pub created_at: i64,
    pub published_at: Option<i64>,
    pub run_id: Option<String>,
}

/// `GET /api/press-pods/episodes/:id` detail: the list fields plus the
/// transcript, chunk stats and itemized costs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PressPodsEpisodeDetail {
    #[serde(flatten)]
    pub episode: PressPodsEpisode,
    pub content: String,
    pub author_gender: Option<String>,
    pub voice_provider: Option<String>,
    pub chunks: Option<Vec<PressPodsChunkStat>>,
    pub costs: Option<PressPodsCosts>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PressPodsJobStatus {
    #[serde(rename = "queued")]
    Queued,
    #[serde(rename = "processing")]
    Processing,
    #[serde(rename = "failed")]
    Failed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PressPodsJob {
    pub job_id: String,
    pub url: String,
    pub status: PressPodsJobStatus,
    pub attempts: i64,
    /// `null` when the job may run immediately (stored `0`).
    pub next_attempt_at: Option<i64>,
    pub last_error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_run_id: Option<String>,
}

/// `GET /api/press-pods/episodes`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PressPodsListResponse {
    pub episodes: Vec<PressPodsEpisode>,
    pub jobs: Vec<PressPodsJob>,
}

/// `GET /api/press-pods/episodes/:id`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PressPodsEpisodeResponse {
    pub episode: PressPodsEpisodeDetail,
}

/// Replies carrying one job (submit, retries).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PressPodsJobResponse {
    pub job: PressPodsJob,
}

/// `POST /pods/episodes` (202).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PressPodsSubmitResponse {
    pub job_id: String,
}

/// `{ "deleted": true }`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PressPodsDeletedResponse {
    pub deleted: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detail_round_trips_the_wire_shape() {
        let wire = serde_json::json!({
            "episode": {
                "episodeId": "e1", "title": "T", "author": null, "publication": "P",
                "domain": "x.com", "articleUrl": "https://x.com/a", "leadImageUrl": null,
                "excerpt": "E", "voiceName": "Higgs (female)", "synthesizedSeconds": 12.5,
                "chapters": [{ "startTimeSeconds": 1.5, "title": "Intro" }],
                "audioUrl": "/pods/audio/e1.mp3", "durationSeconds": 61.2, "fileBytes": 1000,
                "retrieverName": "readability", "retrieverSeconds": 3.2,
                "retrieverAttempts": [
                    { "name": "readability", "success": true, "contentRating": 9, "textChars": 100 },
                    { "name": "fetch", "success": false, "error": "HTTP 500" }
                ],
                "costCents": 1.23, "createdAt": 1767225600000_i64, "publishedAt": null, "runId": null,
                "content": "Body", "authorGender": "female", "voiceProvider": "Higgs",
                "chunks": [{
                    "index": 0, "sectionIndex": 0, "text": "Body", "charCount": 4,
                    "durationSeconds": 2.5, "startTimeSeconds": 1.5, "secPerChar": 0.6,
                    "attempts": 1, "coverage": 1.0, "wordRatio": 1.0, "expectedWords": 1
                }],
                "costs": {
                    "llmCents": 1.0, "ttsCents": 0.23,
                    "detailCents": { "gpt-meta-input": 1.0 },
                    "detailTokens": { "gpt-meta": { "input": 10, "output": 2 } },
                    "detailChars": { "tts": 4 }
                }
            }
        });
        let decoded: PressPodsEpisodeResponse = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(decoded.episode.episode.file_bytes, 1000);
        assert!(matches!(
            decoded.episode.episode.retriever_attempts.as_deref(),
            Some([
                PressPodsRetrieverAttempt::Success { .. },
                PressPodsRetrieverAttempt::Failure { .. }
            ])
        ));
        let back = serde_json::to_value(&decoded).unwrap();
        let reread: PressPodsEpisodeResponse = serde_json::from_value(back).unwrap();
        assert_eq!(reread, decoded);
    }

    #[test]
    fn cost_details_keep_their_insertion_order() {
        let wire = serde_json::json!({
            "llmCents": 1.0, "ttsCents": 0.0,
            "detailCents": { "z-meta-input": 1.0, "a-clean-input": 2.0 },
            "detailTokens": {}, "detailChars": { "tts": 4.0 }
        });
        let costs: PressPodsCosts = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(costs.detail_cents.0[0].0, "z-meta-input");
        assert_eq!(
            serde_json::to_string(&costs).unwrap(),
            serde_json::to_string(&wire).unwrap()
        );
    }

    #[test]
    fn jobs_serialize_status_and_null_next_attempt() {
        let job = PressPodsJob {
            job_id: "j".into(),
            url: "https://a.test".into(),
            status: PressPodsJobStatus::Processing,
            attempts: 1,
            next_attempt_at: None,
            last_error: None,
            created_at: 1,
            updated_at: 2,
            last_run_id: Some("PressPods:r".into()),
        };
        assert_eq!(
            serde_json::to_value(&job).unwrap(),
            serde_json::json!({
                "jobId": "j", "url": "https://a.test", "status": "processing", "attempts": 1,
                "nextAttemptAt": null, "lastError": null, "createdAt": 1, "updatedAt": 2,
                "lastRunId": "PressPods:r"
            })
        );
    }
}
