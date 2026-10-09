//! Persisted PressPods shapes (`src/press-pods/types.ts`, `persistence.ts`).
//!
//! Field order follows the TS object literals that write each row, so a row
//! written by Rust encodes like one written by TS. Every struct carries
//! `extra` so read-modify-write never drops fields a newer writer added.

use indexmap::IndexMap;
use omni_store::Entity;
use omni_store::cbor::Extra;
use serde::{Deserialize, Serialize};

/// `authorGender`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum AuthorGender {
    Male,
    Female,
    Unknown,
}

impl AuthorGender {
    pub fn as_str(self) -> &'static str {
        match self {
            AuthorGender::Male => "male",
            AuthorGender::Female => "female",
            AuthorGender::Unknown => "unknown",
        }
    }
}

/// A chapter marker: title plus its start offset (seconds) into the final audio.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Chapter {
    pub start_time_seconds: f64,
    pub title: String,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Chapter {
    pub fn new(start_time_seconds: f64, title: impl Into<String>) -> Self {
        Self {
            start_time_seconds,
            title: title.into(),
            extra: Extra::new(),
        }
    }
}

/// Per-chunk synthesis stats persisted on the episode for diagnostics.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChunkStat {
    pub index: i64,
    pub section_index: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section_title: Option<String>,
    pub text: String,
    pub char_count: i64,
    pub duration_seconds: f64,
    /// Offset into the final audio (includes the intro jingle), like [`Chapter`].
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
    #[serde(flatten)]
    pub extra: Extra,
}

/// Compact per-retriever outcome persisted on the episode.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RawAttempt", into = "RawAttempt")]
pub enum RetrieverAttempt {
    Success {
        name: String,
        content_rating: f64,
        text_chars: i64,
        extra: Extra,
    },
    Failure {
        name: String,
        error: String,
        extra: Extra,
    },
}

impl RetrieverAttempt {
    pub fn name(&self) -> &str {
        match self {
            RetrieverAttempt::Success { name, .. } | RetrieverAttempt::Failure { name, .. } => name,
        }
    }
}

/// The wire shape of [`RetrieverAttempt`]; `success` discriminates.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawAttempt {
    name: String,
    success: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    content_rating: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    text_chars: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(flatten)]
    extra: Extra,
}

impl TryFrom<RawAttempt> for RetrieverAttempt {
    type Error = String;

    fn try_from(raw: RawAttempt) -> Result<Self, String> {
        if raw.success {
            match (raw.content_rating, raw.text_chars) {
                (Some(content_rating), Some(text_chars)) => Ok(RetrieverAttempt::Success {
                    name: raw.name,
                    content_rating,
                    text_chars,
                    extra: raw.extra,
                }),
                _ => Err("a successful retriever attempt needs contentRating and textChars".into()),
            }
        } else {
            match raw.error {
                Some(error) => Ok(RetrieverAttempt::Failure {
                    name: raw.name,
                    error,
                    extra: raw.extra,
                }),
                None => Err("a failed retriever attempt needs an error".into()),
            }
        }
    }
}

impl From<RetrieverAttempt> for RawAttempt {
    fn from(attempt: RetrieverAttempt) -> Self {
        match attempt {
            RetrieverAttempt::Success {
                name,
                content_rating,
                text_chars,
                extra,
            } => RawAttempt {
                name,
                success: true,
                content_rating: Some(content_rating),
                text_chars: Some(text_chars),
                error: None,
                extra,
            },
            RetrieverAttempt::Failure { name, error, extra } => RawAttempt {
                name,
                success: false,
                content_rating: None,
                text_chars: None,
                error: Some(error),
                extra,
            },
        }
    }
}

/// Token counts per cost detail key.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TokenCounts {
    pub input: f64,
    pub output: f64,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `Costs`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Costs {
    pub llm_cents: f64,
    pub tts_cents: f64,
    pub detail_cents: IndexMap<String, f64>,
    pub detail_tokens: IndexMap<String, TokenCounts>,
    pub detail_chars: IndexMap<String, f64>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `press-pods-episode`, keyed by `episodeId`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PressPodsEpisode {
    /// Random id; doubles as the (unguessable) audio file name stem.
    pub episode_id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_gender: Option<AuthorGender>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publication: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    pub article_url: String,
    /// Canonical identity for dedup/replace; absent on old rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub normalized_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead_image_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub excerpt: Option<String>,
    /// The cleaned, narration-ready text that was synthesized.
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub synthesized_seconds: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chapters: Option<Vec<Chapter>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chunks: Option<Vec<ChunkStat>>,
    pub audio_file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<f64>,
    pub file_bytes: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retriever_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retriever_seconds: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retriever_attempts: Option<Vec<RetrieverAttempt>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub costs: Option<Costs>,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_at: Option<i64>,
    /// Task run that produced this episode; links to its captured logs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity for PressPodsEpisode {
    const NAME: &'static str = "press-pods-episode";
    type Key = String;
    fn key(&self) -> String {
        self.episode_id.clone()
    }
}

/// `PressPodsJobStatus`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Queued,
    Processing,
    Failed,
}

impl JobStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            JobStatus::Queued => "queued",
            JobStatus::Processing => "processing",
            JobStatus::Failed => "failed",
        }
    }
}

/// `press-pods-job`, keyed by `jobId`: the durable submission queue.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PressPodsJob {
    pub job_id: String,
    /// Original submitted URL: what the retrievers fetch (query string intact).
    pub url: String,
    /// Canonical identity for dedup and resubmit-as-retry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub normalized_url: Option<String>,
    pub status: JobStatus,
    pub attempts: i64,
    /// Earliest time the job may run (0 = immediately).
    pub next_attempt_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    /// Set while processing; used to detect crashed runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claimed_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_id: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity for PressPodsJob {
    const NAME: &'static str = "press-pods-job";
    type Key = String;
    fn key(&self) -> String {
        self.job_id.clone()
    }
}
