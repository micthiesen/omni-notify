//! Scoped Arr repairs for Observer issues (`src/observer-repair/arr.ts`).
//!
//! Identifiers come from Arr by exact TMDB/TVDB identity, never from model
//! output or display names. A plan preserves file -> import -> grab provenance
//! and refuses anything that would reach outside the reported scope.

use std::time::Duration;

use omni_http::{HttpClient, Method, Url};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::json_api::{ApiFailure, JsonApi};
use crate::side_effects::SideEffects;

const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArrRepairKind {
    Sonarr,
    Radarr,
}

/// The report's media identity (`issue.media ?? {}`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ArrRepairMedia {
    pub tmdb_id: Option<i64>,
    pub tvdb_id: Option<i64>,
    pub media_type: Option<String>,
}

/// The cause of an [`ArrRepairError`].
#[derive(Debug, thiserror::Error)]
pub enum RepairCause {
    #[error("{0}")]
    Message(String),
    #[error("{0}")]
    Api(#[source] ApiFailure),
}

/// `ArrRepairError`: `"<operation>: <cause>"`.
#[derive(Debug, thiserror::Error)]
#[error("{operation}: {cause}")]
pub struct ArrRepairError {
    pub operation: String,
    #[source]
    pub cause: RepairCause,
}

impl ArrRepairError {
    fn message(operation: &str, message: impl Into<String>) -> Self {
        Self {
            operation: operation.to_owned(),
            cause: RepairCause::Message(message.into()),
        }
    }
}

/// A positive integer id (`Schema.isInt()` and `> 0`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "f64", into = "i64")]
pub struct Id(pub i64);

impl TryFrom<f64> for Id {
    type Error = String;
    fn try_from(value: f64) -> Result<Self, String> {
        #[allow(clippy::cast_possible_truncation)]
        let int = value as i64;
        #[allow(clippy::cast_precision_loss)]
        if value.fract() == 0.0 && int as f64 == value && int > 0 {
            Ok(Id(int))
        } else {
            Err(format!("expected a positive integer id, got {value}"))
        }
    }
}

impl From<Id> for i64 {
    fn from(id: Id) -> i64 {
        id.0
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MediaDto {
    id: Id,
    title: String,
    monitored: bool,
    tmdb_id: Option<f64>,
    tvdb_id: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairEpisode {
    pub id: Id,
    pub series_id: Id,
    pub season_number: i64,
    pub episode_number: i64,
    pub has_file: bool,
    pub monitored: bool,
    pub episode_file_id: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairFile {
    pub id: Id,
    #[serde(default)]
    pub series_id: Option<Id>,
    #[serde(default)]
    pub movie_id: Option<Id>,
    pub path: String,
    #[serde(default)]
    pub scene_name: Option<String>,
}

/// Arr's event type: a name or its numeric enum value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EventType {
    Name(String),
    Number(f64),
}

impl EventType {
    fn is(&self, name: &str, number: f64) -> bool {
        match self {
            EventType::Name(value) => value == name,
            EventType::Number(value) => *value == number,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryData {
    #[serde(default)]
    pub file_id: Option<String>,
    #[serde(default)]
    pub imported_path: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairHistory {
    pub id: Id,
    #[serde(default)]
    pub series_id: Option<Id>,
    #[serde(default)]
    pub movie_id: Option<Id>,
    #[serde(default)]
    pub episode_id: Option<Id>,
    pub source_title: String,
    #[serde(default)]
    pub download_id: Option<String>,
    pub event_type: EventType,
    pub data: HistoryData,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairQueueItem {
    pub id: Id,
    #[serde(default)]
    pub series_id: Option<Id>,
    #[serde(default)]
    pub movie_id: Option<Id>,
    #[serde(default)]
    pub episode_id: Option<Id>,
    #[serde(default)]
    pub download_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct QueuePage {
    total_records: f64,
    records: Vec<RepairQueueItem>,
}

#[derive(Deserialize)]
struct CommandDto {
    id: Id,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BlocklistRecord {
    source_title: String,
    #[serde(default)]
    series_id: Option<Id>,
    #[serde(default)]
    movie_id: Option<Id>,
}

#[derive(Deserialize)]
struct BlocklistPage {
    records: Vec<BlocklistRecord>,
}

/// The current Arr state of the reported title.
#[derive(Clone, Debug, PartialEq)]
pub struct ArrInspection {
    pub kind: ArrRepairKind,
    pub id: i64,
    pub title: String,
    pub monitored: bool,
    pub episodes: Vec<RepairEpisode>,
    pub files: Vec<RepairFile>,
    pub history: Vec<RepairHistory>,
    pub queue: Vec<RepairQueueItem>,
}

/// An executable, verified-in-advance repair.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArrRepairScope {
    pub kind: ArrRepairKind,
    pub id: i64,
    pub title: String,
    pub description: String,
    pub episode_ids: Vec<i64>,
    pub file_ids: Vec<i64>,
    pub history_ids: Vec<i64>,
    pub releases: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepairInstructionAction {
    Replace,
    SearchMissing,
}

/// What the validated decision asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArrRepairInstruction {
    pub action: RepairInstructionAction,
    pub season: Option<i64>,
    pub episodes: Vec<i64>,
}

#[derive(Clone, Debug)]
pub struct ArrRepairClientConfig {
    pub kind: ArrRepairKind,
    pub url: String,
    pub api_key: String,
    pub http: HttpClient,
    pub side_effects: SideEffects,
}

/// Resolves identifiers from Arr, never from model output or display-name matching.
#[derive(Clone, Debug)]
pub struct ArrRepairClient {
    kind: ArrRepairKind,
    base: Url,
    api: JsonApi,
}

impl ArrRepairClient {
    pub fn new(config: ArrRepairClientConfig) -> Result<Self, ArrRepairError> {
        let base = Url::parse(&format!("{}/", config.url.trim_end_matches('/')))
            .and_then(|root| root.join("api/v3/"))
            .map_err(|e| {
                ArrRepairError::message("configure Arr client", format!("invalid URL: {e}"))
            })?;
        Ok(Self {
            kind: config.kind,
            base,
            api: JsonApi::new(
                config.http,
                config.api_key,
                config.side_effects,
                MAX_RESPONSE_BYTES,
                REQUEST_TIMEOUT,
            ),
        })
    }

    pub fn kind(&self) -> ArrRepairKind {
        self.kind
    }

    fn url(&self, operation: &str, path: &str) -> Result<Url, ArrRepairError> {
        self.base.join(path).map_err(|e| ArrRepairError {
            operation: operation.to_owned(),
            cause: RepairCause::Api(ApiFailure::Url(e.to_string())),
        })
    }

    async fn request<T: DeserializeOwned>(
        &self,
        operation: &str,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<T, ArrRepairError> {
        let url = self.url(operation, path)?;
        self.api
            .json(method, url, body)
            .await
            .map_err(|cause| ArrRepairError {
                operation: operation.to_owned(),
                cause: RepairCause::Api(cause),
            })
    }

    async fn request_void(
        &self,
        operation: &str,
        method: Method,
        path: &str,
    ) -> Result<(), ArrRepairError> {
        let url = self.url(operation, path)?;
        self.api
            .void(method, url)
            .await
            .map_err(|cause| ArrRepairError {
                operation: operation.to_owned(),
                cause: RepairCause::Api(cause),
            })
    }

    fn relation(&self) -> &'static str {
        match self.kind {
            ArrRepairKind::Radarr => "movieId",
            ArrRepairKind::Sonarr => "seriesId",
        }
    }

    /// Finds the exact title by TMDB (movies) or TVDB (series) id and reads its
    /// episodes, files, history and active downloads.
    pub async fn inspect(&self, media: &ArrRepairMedia) -> Result<ArrInspection, ArrRepairError> {
        let movie = self.kind == ArrRepairKind::Radarr;
        let external_id = if movie { media.tmdb_id } else { media.tvdb_id };
        let expected_type = if movie { "movie" } else { "tv" };
        let external_id = match external_id {
            Some(id) if id != 0 && media.media_type.as_deref() == Some(expected_type) => id,
            _ => {
                return Err(ArrRepairError::message(
                    "identify title",
                    "Missing matching TMDB/TVDB identity",
                ));
            }
        };
        let key = if movie { "tmdbId" } else { "tvdbId" };
        let resource = if movie { "movie" } else { "series" };
        #[allow(clippy::cast_precision_loss)]
        let wanted = external_id as f64;
        let matches: Vec<MediaDto> = self
            .request::<Vec<MediaDto>>(
                "identify title",
                Method::GET,
                &format!("{resource}?{key}={external_id}"),
                None,
            )
            .await?
            .into_iter()
            .filter(|item| {
                let id = if movie { item.tmdb_id } else { item.tvdb_id };
                id == Some(wanted)
            })
            .collect();
        let [target] = matches.as_slice() else {
            return Err(ArrRepairError::message(
                "identify title",
                "Exact title is missing or ambiguous",
            ));
        };
        let target_id = target.id.0;
        let relation = self.relation();
        let episodes: Vec<RepairEpisode> = if movie {
            Vec::new()
        } else {
            self.request(
                "inspect episodes",
                Method::GET,
                &format!("episode?seriesId={target_id}"),
                None,
            )
            .await?
        };
        let files: Vec<RepairFile> = self
            .request(
                "inspect files",
                Method::GET,
                &format!(
                    "{}?{relation}={target_id}",
                    if movie { "moviefile" } else { "episodefile" }
                ),
                None,
            )
            .await?;
        let history: Vec<RepairHistory> = self
            .request(
                "inspect history",
                Method::GET,
                &format!("history/{resource}?{relation}={target_id}"),
                None,
            )
            .await?;
        let queue = self.queue().await?;
        if episodes.len() > 2000 || history.len() > 10000 || files.len() > 2000 {
            return Err(ArrRepairError::message(
                "inspect title",
                "Title exceeds bounded repair inspection",
            ));
        }
        let related = |series: Option<Id>, movie_id: Option<Id>| {
            let id = if movie { movie_id } else { series };
            id == Some(Id(target_id))
        };
        Ok(ArrInspection {
            kind: self.kind,
            id: target_id,
            title: target.title.clone(),
            monitored: target.monitored,
            episodes: episodes
                .into_iter()
                .filter(|e| e.series_id == Id(target_id))
                .collect(),
            files: files
                .into_iter()
                .filter(|f| related(f.series_id, f.movie_id))
                .collect(),
            history: history
                .into_iter()
                .filter(|h| related(h.series_id, h.movie_id))
                .collect(),
            queue: queue
                .into_iter()
                .filter(|q| related(q.series_id, q.movie_id))
                .collect(),
        })
    }

    async fn queue(&self) -> Result<Vec<RepairQueueItem>, ArrRepairError> {
        let page: QueuePage = self
            .request(
                "inspect queue",
                Method::GET,
                "queue?page=1&pageSize=1000",
                None,
            )
            .await?;
        #[allow(clippy::cast_precision_loss)]
        if page.total_records > page.records.len() as f64 {
            return Err(ArrRepairError::message(
                "inspect queue",
                "Queue exceeds inspection bound",
            ));
        }
        Ok(page.records)
    }

    /// Plans a scoped repair from a fresh inspection, or explains why it is unsafe.
    pub fn plan(
        &self,
        inspected: &ArrInspection,
        instruction: &ArrRepairInstruction,
    ) -> Result<ArrRepairScope, ArrRepairError> {
        plan(self.kind, inspected, instruction)
            .map_err(|message| ArrRepairError::message("plan repair", message))
    }

    /// Blocklists the grabbed releases, verifies the blocklist, deletes the scoped
    /// files and verifies their deletion.
    pub async fn blocklist_and_delete(&self, scope: &ArrRepairScope) -> Result<(), ArrRepairError> {
        for id in &scope.history_ids {
            self.request_void(
                "blocklist release",
                Method::POST,
                &format!("history/failed/{id}"),
            )
            .await?;
        }
        if !scope.releases.is_empty() {
            let blocklist: BlocklistPage = self
                .request(
                    "verify blocklist",
                    Method::GET,
                    "blocklist?page=1&pageSize=1000&sortKey=date&sortDirection=descending",
                    None,
                )
                .await?;
            let missing = scope.releases.iter().any(|title| {
                !blocklist.records.iter().any(|record| {
                    let owner = match scope.kind {
                        ArrRepairKind::Sonarr => record.series_id,
                        ArrRepairKind::Radarr => record.movie_id,
                    };
                    record.source_title == *title && owner == Some(Id(scope.id))
                })
            });
            if missing {
                return Err(ArrRepairError::message(
                    "verify blocklist",
                    "Release not visible in blocklist",
                ));
            }
        }
        let resource = match scope.kind {
            ArrRepairKind::Sonarr => "episodefile",
            ArrRepairKind::Radarr => "moviefile",
        };
        for id in &scope.file_ids {
            self.request_void(
                "delete media file",
                Method::DELETE,
                &format!("{resource}/{id}"),
            )
            .await?;
        }
        let relation = match scope.kind {
            ArrRepairKind::Sonarr => "seriesId",
            ArrRepairKind::Radarr => "movieId",
        };
        let remaining: Vec<RepairFile> = self
            .request(
                "verify file deletion",
                Method::GET,
                &format!("{resource}?{relation}={}", scope.id),
                None,
            )
            .await?;
        if remaining
            .iter()
            .any(|file| scope.file_ids.contains(&file.id.0))
        {
            return Err(ArrRepairError::message(
                "verify file deletion",
                "Deleted file is still present",
            ));
        }
        Ok(())
    }

    /// Starts Arr's automatic search; returns the accepted command id.
    pub async fn search(&self, scope: &ArrRepairScope) -> Result<i64, ArrRepairError> {
        let body = match scope.kind {
            ArrRepairKind::Sonarr => {
                json!({ "name": "EpisodeSearch", "episodeIds": scope.episode_ids })
            }
            ArrRepairKind::Radarr => json!({ "name": "MoviesSearch", "movieIds": [scope.id] }),
        };
        let command: CommandDto = self
            .request(
                "start automatic search",
                Method::POST,
                "command",
                Some(body),
            )
            .await?;
        Ok(command.id.0)
    }

    /// Blocklist, delete and verify, then search.
    pub async fn replace(&self, scope: &ArrRepairScope) -> Result<i64, ArrRepairError> {
        self.blocklist_and_delete(scope).await?;
        self.search(scope).await
    }
}

/// The pure planning rules (`ArrRepairClient.plan`).
pub fn plan(
    kind: ArrRepairKind,
    inspected: &ArrInspection,
    instruction: &ArrRepairInstruction,
) -> Result<ArrRepairScope, String> {
    let fail = |message: &str| Err(message.to_owned());
    if inspected.kind != kind || !inspected.monitored {
        return fail("Unmonitored or mismatched title");
    }
    let sonarr = inspected.kind == ArrRepairKind::Sonarr;
    let wanted: Vec<&RepairEpisode> = inspected
        .episodes
        .iter()
        .filter(|e| {
            (match instruction.season {
                None => e.season_number > 0,
                Some(season) => e.season_number == season,
            }) && (instruction.episodes.is_empty()
                || instruction.episodes.contains(&e.episode_number))
        })
        .collect();
    if sonarr {
        if wanted.is_empty() {
            return fail("Requested episode scope is empty");
        }
        if instruction
            .episodes
            .iter()
            .any(|n| !wanted.iter().any(|e| e.episode_number == *n))
        {
            return fail("One or more requested episodes do not exist");
        }
    }
    let search_missing = instruction.action == RepairInstructionAction::SearchMissing;
    let targets: Vec<&RepairEpisode> = if search_missing {
        wanted.into_iter().filter(|e| !e.has_file).collect()
    } else {
        wanted
    };
    if targets.iter().any(|e| !e.monitored) {
        return fail("Requested scope includes unmonitored episodes");
    }
    if sonarr && targets.is_empty() {
        return fail("Requested episodes are already present");
    }
    if search_missing && !sonarr && !inspected.files.is_empty() {
        return fail("Movie is already present");
    }
    let episode_ids: Vec<i64> = targets.iter().map(|e| e.id.0).collect();
    // Queue removal can affect a complete season pack. Leave active downloads to Arr recovery.
    if inspected.queue.iter().any(|q| {
        !sonarr
            || q.episode_id
                .is_none_or(|episode| episode_ids.contains(&episode.0))
    }) {
        return fail("An active download overlaps the requested scope");
    }
    let file_ids: Vec<i64> = if search_missing {
        Vec::new()
    } else if !sonarr {
        inspected.files.iter().map(|f| f.id.0).collect()
    } else {
        let mut ids: Vec<i64> = Vec::new();
        for episode in targets.iter().filter(|e| e.has_file) {
            if !ids.contains(&episode.episode_file_id) {
                ids.push(episode.episode_file_id);
            }
        }
        ids
    };
    let mut history_ids: Vec<i64> = Vec::new();
    let mut releases: Vec<String> = Vec::new();
    for file_id in &file_ids {
        let Some(file) = inspected.files.iter().find(|f| f.id.0 == *file_id) else {
            return fail("Current file mapping is incomplete");
        };
        if inspected
            .episodes
            .iter()
            .any(|e| e.episode_file_id == *file_id && !episode_ids.contains(&e.id.0))
        {
            return fail("File contains episodes outside the requested scope");
        }
        let file_id_text = file_id.to_string();
        let mut downloads: Vec<&str> = Vec::new();
        for import in inspected.history.iter().filter(|h| {
            h.event_type.is("downloadFolderImported", 3.0)
                && (h.data.file_id.as_deref() == Some(file_id_text.as_str())
                    || (h.data.imported_path.as_deref() == Some(file.path.as_str())
                        && file.scene_name.as_deref() == Some(h.source_title.as_str())))
        }) {
            if let Some(download) = import.download_id.as_deref().filter(|d| !d.is_empty())
                && !downloads.contains(&download)
            {
                downloads.push(download);
            }
        }
        let [download] = downloads.as_slice() else {
            return fail("Current file lacks unambiguous import history");
        };
        let grabs: Vec<&RepairHistory> = inspected
            .history
            .iter()
            .filter(|h| {
                h.event_type.is("grabbed", 1.0) && h.download_id.as_deref() == Some(*download)
            })
            .collect();
        let Some(grab) = grabs.first() else {
            return fail("Current file lacks matching grabbed release history");
        };
        if sonarr
            && grabs
                .iter()
                .any(|h| h.episode_id.is_none_or(|e| !episode_ids.contains(&e.0)))
        {
            return fail("Blocklisting the release would also re-search episodes outside scope");
        }
        if !releases.contains(&grab.source_title) {
            history_ids.push(grab.id.0);
            releases.push(grab.source_title.clone());
        }
    }
    let scope_text = if !sonarr {
        "movie".to_owned()
    } else {
        match instruction.season {
            None => "series".to_owned(),
            Some(season) if instruction.episodes.is_empty() => format!("season {season}"),
            Some(season) => format!(
                "season {season} episodes {}",
                instruction
                    .episodes
                    .iter()
                    .map(i64::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    };
    let outcome = if search_missing {
        "Searched missing files only.".to_owned()
    } else {
        format!(
            "Blocklisted {} release(s) and removed {} file(s).",
            history_ids.len(),
            file_ids.len()
        )
    };
    Ok(ArrRepairScope {
        kind: inspected.kind,
        id: inspected.id,
        title: inspected.title.clone(),
        description: format!("{}: {scope_text}. {outcome}", inspected.title),
        episode_ids,
        file_ids,
        history_ids,
        releases,
    })
}
