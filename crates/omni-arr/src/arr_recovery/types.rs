//! Arr recovery domain types (`src/arr-recovery/types.ts`).

use std::future::Future;

use omni_ai::AiError;
use omni_alerts::PushoverError;
use omni_http::HttpError;
use omni_store::StoreError;
use serde::{Deserialize, Serialize};

use crate::json_api::ApiFailure;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArrKind {
    Sonarr,
    Radarr,
}

impl ArrKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ArrKind::Sonarr => "sonarr",
            ArrKind::Radarr => "radarr",
        }
    }
}

impl std::fmt::Display for ArrKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// `ArrRecoveryError`: `"<operation>: <cause>"`.
#[derive(Debug, thiserror::Error)]
#[error("{operation}: {cause}")]
pub struct ArrRecoveryError {
    pub operation: String,
    #[source]
    pub cause: ArrCause,
}

/// The cause of an [`ArrRecoveryError`].
#[derive(Debug, thiserror::Error)]
pub enum ArrCause {
    #[error("{0}")]
    Message(String),
    #[error("{0}")]
    Api(#[source] ApiFailure),
    #[error("{0}")]
    Http(#[source] HttpError),
    #[error("{0}")]
    Pushover(#[source] PushoverError),
    #[error("{0}")]
    Store(#[source] StoreError),
    #[error("{0}")]
    Ai(#[source] AiError),
    #[error("{0}")]
    Nested(#[source] Box<ArrRecoveryError>),
    #[error("timed out after {0} seconds")]
    Timeout(u64),
}

impl ArrRecoveryError {
    pub fn new(operation: impl Into<String>, cause: ArrCause) -> Self {
        Self {
            operation: operation.into(),
            cause,
        }
    }

    pub fn message(operation: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(operation, ArrCause::Message(message.into()))
    }

    /// Wraps `inner` under a new operation (TS `new ArrRecoveryError({ operation, cause })`).
    pub fn wrap(operation: impl Into<String>, inner: ArrRecoveryError) -> Self {
        Self::new(operation, ArrCause::Nested(Box::new(inner)))
    }

    /// A confirmed Pushover rejection (HTTP 4xx): the batch was not accepted.
    pub fn is_definite_pushover_rejection(&self) -> bool {
        match &self.cause {
            ArrCause::Pushover(error) => error.is_definite_rejection(),
            _ => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusMessage {
    pub title: String,
    pub messages: Vec<String>,
}

/// One queue record with a download id.
#[derive(Clone, Debug, PartialEq)]
pub struct QueueItem {
    pub id: i64,
    pub download_id: String,
    pub title: String,
    pub status: String,
    pub tracked_download_status: String,
    pub tracked_download_state: String,
    pub status_messages: Vec<StatusMessage>,
    pub size: f64,
    pub sizeleft: f64,
    pub output_path: Option<String>,
    pub added: Option<String>,
    pub series_id: Option<i64>,
    pub episode_id: Option<i64>,
    pub movie_id: Option<i64>,
    pub protocol: Option<String>,
    pub download_client: Option<String>,
}

impl QueueItem {
    /// A queue item that only names a target (reconcile's target refresh).
    pub fn target_ref(
        series_id: Option<i64>,
        episode_id: Option<i64>,
        movie_id: Option<i64>,
    ) -> Self {
        Self {
            id: 0,
            download_id: String::new(),
            title: String::new(),
            status: String::new(),
            tracked_download_status: String::new(),
            tracked_download_state: String::new(),
            status_messages: Vec::new(),
            size: 0.0,
            sizeleft: 0.0,
            output_path: None,
            added: None,
            series_id,
            episode_id,
            movie_id,
            protocol: None,
            download_client: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetEpisode {
    pub id: i64,
    pub season_number: i64,
    pub episode_number: i64,
    pub title: String,
    pub has_file: bool,
    pub monitored: bool,
}

/// The movie or the series episodes a queue group is for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    pub id: i64,
    pub title: String,
    pub year: i64,
    pub monitored: bool,
    pub has_file: bool,
    pub path: String,
    pub episode_ids: Vec<i64>,
    pub episodes: Vec<TargetEpisode>,
    pub alternate_titles: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Language {
    pub id: i64,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rejection {
    pub reason: String,
    #[serde(rename = "type")]
    pub kind: String,
}

/// One manual-import preview file (persisted in reservations).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_name: Option<String>,
    pub id: i64,
    pub path: String,
    pub name: String,
    pub size: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub movie_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub season_number: Option<i64>,
    pub episode_ids: Vec<i64>,
    /// The full Arr quality object, sent back verbatim on import.
    pub quality: serde_json::Map<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub languages: Option<Vec<Language>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_group: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub indexer_flags: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_type: Option<String>,
    pub rejections: Vec<Rejection>,
}

/// One grab history record.
#[derive(Clone, Debug, PartialEq)]
pub struct Grab {
    pub download_id: String,
    pub source_title: String,
    pub series_id: Option<i64>,
    pub movie_id: Option<i64>,
    pub episode_id: Option<i64>,
    pub event_type: String,
    pub date: String,
}

/// Everything known about one queue group.
#[derive(Clone, Debug, PartialEq)]
pub struct Evidence {
    pub download_health: Option<String>,
    pub kind: ArrKind,
    pub items: Vec<QueueItem>,
    pub target: Target,
    pub files: Vec<ImportFile>,
    pub grabs: Vec<Grab>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DecisionSource {
    Rules,
    Llm,
}

/// What to do with a queue group.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "lowercase")]
pub enum Decision {
    Import {
        reason: String,
        source: DecisionSource,
    },
    Remove {
        reason: String,
        source: DecisionSource,
        replace: bool,
    },
    Defer {
        reason: String,
        source: DecisionSource,
    },
}

impl Decision {
    pub fn defer(reason: impl Into<String>, source: DecisionSource) -> Self {
        Decision::Defer {
            reason: reason.into(),
            source,
        }
    }

    pub fn action(&self) -> &'static str {
        match self {
            Decision::Import { .. } => "import",
            Decision::Remove { .. } => "remove",
            Decision::Defer { .. } => "defer",
        }
    }

    pub fn reason(&self) -> &str {
        match self {
            Decision::Import { reason, .. }
            | Decision::Remove { reason, .. }
            | Decision::Defer { reason, .. } => reason,
        }
    }

    pub fn source(&self) -> DecisionSource {
        match self {
            Decision::Import { source, .. }
            | Decision::Remove { source, .. }
            | Decision::Defer { source, .. } => *source,
        }
    }

    /// `decision.action === "remove" && decision.replace`.
    pub fn is_replacement(&self) -> bool {
        matches!(self, Decision::Remove { replace: true, .. })
    }
}

/// An Arr command's state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandStatus {
    pub status: String,
    pub message: Option<String>,
}

pub type ArrResult<T> = Result<T, ArrRecoveryError>;

/// The Sonarr/Radarr operations recovery needs (`ArrClient`).
pub trait ArrClient: Send + Sync {
    fn kind(&self) -> ArrKind;
    fn queue(&self) -> impl Future<Output = ArrResult<Vec<QueueItem>>> + Send;
    fn preview(&self, download_id: &str)
    -> impl Future<Output = ArrResult<Vec<ImportFile>>> + Send;
    fn target(&self, items: &[QueueItem]) -> impl Future<Output = ArrResult<Target>> + Send;
    fn history(&self, download_id: &str) -> impl Future<Output = ArrResult<Vec<Grab>>> + Send;
    fn import_files(
        &self,
        download_id: &str,
        files: &[ImportFile],
    ) -> impl Future<Output = ArrResult<i64>> + Send;
    fn command(&self, id: i64) -> impl Future<Output = ArrResult<CommandStatus>> + Send;
    fn remove(&self, id: i64, blocklist: bool) -> impl Future<Output = ArrResult<()>> + Send;
    fn verify_removed(&self, output_path: &str) -> impl Future<Output = ArrResult<bool>> + Send;
    fn search(&self, target: &Target) -> impl Future<Output = ArrResult<i64>> + Send;
    fn verify_imported(
        &self,
        target: &Target,
        files: &[ImportFile],
    ) -> impl Future<Output = ArrResult<bool>> + Send;
}
