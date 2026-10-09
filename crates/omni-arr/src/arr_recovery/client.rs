//! Sonarr/Radarr v3 API client for recovery.

use std::sync::LazyLock;
use std::time::Duration;

use omni_http::{HttpClient, Method, Url};
use regex::Regex;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};

use super::filesystem;
use super::types::{
    ArrCause, ArrClient, ArrKind, ArrRecoveryError, ArrResult, CommandStatus, Grab, ImportFile,
    Language, QueueItem, Rejection, StatusMessage, Target, TargetEpisode,
};
use crate::json_api::{ApiFailure, JsonApi};
use crate::side_effects::SideEffects;

const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const PAGE_SIZE: usize = 250;
const MAX_PAGES: usize = 20;
const MAX_RECORDS: usize = PAGE_SIZE * MAX_PAGES;

/// A JSON value Arr sends as either a string or a number.
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
enum StrOrNum {
    Str(String),
    Num(f64),
}

/// TS `valueString`: `null`/`undefined` become `""`, numbers JS-formatted.
fn value_string(value: Option<&StrOrNum>) -> String {
    match value {
        None => String::new(),
        Some(StrOrNum::Str(s)) => s.clone(),
        Some(StrOrNum::Num(n)) => omni_core::js::number_to_string(*n),
    }
}

fn non_empty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageDto<A> {
    #[allow(dead_code)]
    page: f64,
    page_size: f64,
    total_records: f64,
    records: Vec<A>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct QueueRecordDto {
    id: i64,
    download_id: Option<String>,
    title: String,
    status: StrOrNum,
    tracked_download_status: Option<StrOrNum>,
    tracked_download_state: Option<StrOrNum>,
    status_messages: Option<Vec<StatusMessage>>,
    size: f64,
    sizeleft: Option<f64>,
    #[serde(rename = "sizeLeft")]
    size_left: Option<f64>,
    output_path: Option<String>,
    added: Option<String>,
    series_id: Option<i64>,
    episode_id: Option<i64>,
    movie_id: Option<i64>,
    protocol: Option<StrOrNum>,
    download_client: Option<String>,
}

#[derive(Deserialize)]
struct IdDto {
    id: i64,
}

#[derive(Deserialize)]
struct RejectionDto {
    reason: String,
    #[serde(rename = "type")]
    kind: StrOrNum,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PreviewDto {
    id: i64,
    path: String,
    folder_name: Option<String>,
    name: String,
    size: f64,
    series: Option<IdDto>,
    movie: Option<IdDto>,
    season_number: Option<i64>,
    episodes: Option<Vec<IdDto>>,
    quality: Map<String, Value>,
    languages: Option<Vec<Language>>,
    release_group: Option<String>,
    indexer_flags: Option<i64>,
    release_type: Option<StrOrNum>,
    rejections: Option<Vec<RejectionDto>>,
}

/// `QualityCoreSchema`: the quality object must carry these to be importable.
#[derive(Deserialize)]
#[allow(dead_code)]
struct QualityCoreDto {
    quality: QualityNameDto,
    revision: QualityRevisionDto,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct QualityNameDto {
    id: f64,
    name: String,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct QualityRevisionDto {
    version: f64,
    real: f64,
}

#[derive(Deserialize)]
struct TitleDto {
    title: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SeriesDto {
    id: i64,
    title: String,
    year: i64,
    monitored: bool,
    path: String,
    alternate_titles: Option<Vec<TitleDto>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EpisodeDto {
    id: i64,
    season_number: i64,
    episode_number: i64,
    title: String,
    has_file: bool,
    monitored: bool,
    episode_file_id: Option<i64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MovieDto {
    id: i64,
    title: String,
    year: i64,
    monitored: bool,
    path: String,
    has_file: Option<bool>,
    movie_file_id: Option<i64>,
    alternate_titles: Option<Vec<TitleDto>>,
}

impl MovieDto {
    /// `movie.hasFile ?? (movie.movieFileId ?? 0) > 0`.
    fn has_file(&self) -> bool {
        self.has_file
            .unwrap_or_else(|| self.movie_file_id.unwrap_or(0) > 0)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HistoryDto {
    download_id: String,
    source_title: String,
    series_id: Option<i64>,
    movie_id: Option<i64>,
    episode_id: Option<i64>,
    event_type: StrOrNum,
    date: String,
}

#[derive(Deserialize)]
struct CommandDto {
    id: i64,
    status: StrOrNum,
    message: Option<String>,
}

/// A file Arr reports as linked to the target (episode or movie file).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportedFileDto {
    id: i64,
    size: Option<f64>,
    movie_id: Option<i64>,
    path: String,
    relative_path: String,
    scene_name: Option<String>,
    original_file_path: Option<String>,
}

#[derive(Deserialize)]
struct PathDto {
    path: String,
}

#[derive(Deserialize)]
struct FileSystemDto {
    directories: Vec<PathDto>,
    files: Vec<PathDto>,
}

/// `normalizedPath`: forward slashes, no trailing slash, lower case.
pub fn normalized_path(value: &str) -> String {
    value
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_lowercase()
}

static VIDEO_EXTENSION: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\.(?:3g2|3gp|asf|avi|flv|m2ts|m4v|mkv|mov|mp4|mpeg|mpg|mts|ogm|ogv|ts|webm|wmv)$",
    )
    .ok()
});

/// `baseNameWithoutExtension`.
pub fn base_name_without_extension(value: &str) -> String {
    let normalized = normalized_path(value);
    let name = normalized.rsplit('/').next().unwrap_or_default();
    match VIDEO_EXTENSION.as_ref() {
        Some(re) => re.replace(name, "").into_owned(),
        None => name.to_owned(),
    }
}

fn source_matches_file(source: &ImportFile, imported: &ImportedFileDto) -> bool {
    let source_path = normalized_path(&source.path);
    let source_name = base_name_without_extension(if source.name.is_empty() {
        &source.path
    } else {
        &source.name
    });
    let imported_names: Vec<String> = [
        imported.scene_name.as_deref(),
        Some(imported.path.as_str()),
        Some(imported.relative_path.as_str()),
        imported.original_file_path.as_deref(),
    ]
    .into_iter()
    .flatten()
    .filter(|value| !value.is_empty())
    .map(base_name_without_extension)
    .collect();

    normalized_path(&imported.path) == source_path
        || normalized_path(imported.original_file_path.as_deref().unwrap_or_default())
            == source_path
        || imported_names.contains(&source_name)
        // For obfuscated files Arr records the release folder as sceneName. Require
        // the exact byte size as well as the already-verified target file link.
        || match (&source.folder_name, &imported.scene_name) {
            (Some(folder), Some(scene)) => {
                source.size > 0.0
                    && imported.size == Some(source.size)
                    && normalized_path(scene) == normalized_path(folder)
            }
            _ => false,
        }
}

/// How an [`HttpArrClient`] is built.
#[derive(Clone, Debug)]
pub struct ArrClientConfig {
    pub kind: ArrKind,
    pub url: String,
    pub api_key: String,
    pub http: HttpClient,
    pub side_effects: SideEffects,
    /// `ARR_RECOVERY_LOCAL_FILES`: verify deletions on the read-only download mounts.
    pub local_files: bool,
}

/// The production [`ArrClient`].
#[derive(Clone, Debug)]
pub struct HttpArrClient {
    kind: ArrKind,
    base: Url,
    api: JsonApi,
    local_files: bool,
}

fn op_error(operation: &str, failure: ApiFailure) -> ArrRecoveryError {
    ArrRecoveryError::new(operation, ArrCause::Api(failure))
}

impl HttpArrClient {
    pub fn new(config: ArrClientConfig) -> Result<Self, ArrRecoveryError> {
        let base = Url::parse(&format!("{}/", config.url.trim_end_matches('/')))
            .and_then(|root| root.join("api/v3/"))
            .map_err(|e| {
                ArrRecoveryError::message("configure Arr client", format!("invalid URL: {e}"))
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
            local_files: config.local_files,
        })
    }

    fn endpoint(&self, operation: &str, path: &str, query: &[(&str, &str)]) -> ArrResult<Url> {
        let mut url = self
            .base
            .join(path)
            .map_err(|e| op_error(operation, ApiFailure::Url(e.to_string())))?;
        if !query.is_empty() {
            url.query_pairs_mut().extend_pairs(query);
        }
        Ok(url)
    }

    async fn get<T: DeserializeOwned>(
        &self,
        operation: &str,
        path: &str,
        query: &[(&str, &str)],
    ) -> ArrResult<T> {
        let url = self.endpoint(operation, path, query)?;
        self.api
            .json(Method::GET, url, None)
            .await
            .map_err(|failure| op_error(operation, failure))
    }

    async fn post<T: DeserializeOwned>(
        &self,
        operation: &str,
        path: &str,
        body: Value,
    ) -> ArrResult<T> {
        let url = self.endpoint(operation, path, &[])?;
        self.api
            .json(Method::POST, url, Some(body))
            .await
            .map_err(|failure| op_error(operation, failure))
    }

    async fn read_pages<A: DeserializeOwned>(
        &self,
        operation: &str,
        path: &str,
        query: &[(&str, &str)],
    ) -> ArrResult<Vec<A>> {
        let mut records = Vec::new();
        let page_size = PAGE_SIZE.to_string();
        for page in 1..=MAX_PAGES {
            let page = page.to_string();
            let mut paged = query.to_vec();
            paged.push(("page", page.as_str()));
            paged.push(("pageSize", page_size.as_str()));
            let result: PageDto<A> = self.get(operation, path, &paged).await?;
            let received = result.records.len();
            records.extend(result.records);
            if records.len() > MAX_RECORDS {
                return Err(ArrRecoveryError::message(
                    operation,
                    "record limit exceeded",
                ));
            }
            #[allow(clippy::cast_precision_loss)]
            let (count, received_f) = (records.len() as f64, received as f64);
            if received == 0 || count >= result.total_records || received_f < result.page_size {
                return Ok(records);
            }
        }
        Err(ArrRecoveryError::message(operation, "page limit exceeded"))
    }

    fn map_preview(raw: PreviewDto) -> ArrResult<ImportFile> {
        serde_json::from_value::<QualityCoreDto>(Value::Object(raw.quality.clone()))
            .map_err(|e| op_error("decode manual import quality", ApiFailure::Json(e)))?;
        Ok(ImportFile {
            folder_name: raw.folder_name.filter(|name| !name.is_empty()),
            id: raw.id,
            path: raw.path,
            name: raw.name,
            size: raw.size,
            series_id: raw.series.map(|s| s.id),
            movie_id: raw.movie.map(|m| m.id),
            season_number: raw.season_number,
            episode_ids: raw
                .episodes
                .unwrap_or_default()
                .into_iter()
                .map(|e| e.id)
                .collect(),
            quality: raw.quality,
            languages: raw.languages,
            release_group: raw.release_group,
            indexer_flags: raw.indexer_flags,
            release_type: non_empty(value_string(raw.release_type.as_ref())),
            rejections: raw
                .rejections
                .unwrap_or_default()
                .into_iter()
                .map(|r| Rejection {
                    reason: r.reason,
                    kind: value_string(Some(&r.kind)),
                })
                .collect(),
        })
    }

    async fn verify_removed_via_api(&self, output_path: &str) -> ArrResult<bool> {
        const OPERATION: &str = "verify removed data";
        let cleaned = output_path.replace('\\', "/");
        let cleaned = cleaned.trim_end_matches('/');
        let normalized = normalized_path(output_path);
        let slash = cleaned.rfind('/');
        let parent = match slash {
            Some(index) if index > 0 && !normalized.is_empty() => &cleaned[..index],
            _ => return Err(ArrRecoveryError::message(OPERATION, "invalid output path")),
        };
        let contents: FileSystemDto = self
            .get(
                OPERATION,
                "filesystem",
                &[
                    ("path", parent),
                    ("includeFiles", "true"),
                    ("allowFoldersWithoutTrailingSlashes", "true"),
                ],
            )
            .await?;
        let entities: Vec<&PathDto> = contents
            .directories
            .iter()
            .chain(contents.files.iter())
            .collect();
        if entities
            .iter()
            .any(|entity| normalized_path(&entity.path) == normalized)
        {
            return Ok(false);
        }
        // Arr deliberately maps missing, inaccessible, and empty directories to
        // the same empty response. Require another entry as positive evidence
        // that the parent directory was readable.
        Ok(!entities.is_empty())
    }
}

impl ArrClient for HttpArrClient {
    fn kind(&self) -> ArrKind {
        self.kind
    }

    async fn queue(&self) -> ArrResult<Vec<QueueItem>> {
        let records: Vec<QueueRecordDto> = self.read_pages("read queue", "queue", &[]).await?;
        Ok(records
            .into_iter()
            .filter_map(|record| {
                let download_id = record.download_id.filter(|id| !id.is_empty())?;
                Some(QueueItem {
                    id: record.id,
                    download_id,
                    title: record.title,
                    status: value_string(Some(&record.status)),
                    tracked_download_status: value_string(record.tracked_download_status.as_ref()),
                    tracked_download_state: value_string(record.tracked_download_state.as_ref()),
                    status_messages: record.status_messages.unwrap_or_default(),
                    size: record.size,
                    sizeleft: record.sizeleft.or(record.size_left).unwrap_or(0.0),
                    output_path: record.output_path,
                    added: record.added,
                    series_id: record.series_id,
                    episode_id: record.episode_id,
                    movie_id: record.movie_id,
                    protocol: non_empty(value_string(record.protocol.as_ref())),
                    download_client: record.download_client,
                })
            })
            .collect())
    }

    async fn preview(&self, download_id: &str) -> ArrResult<Vec<ImportFile>> {
        let records: Vec<PreviewDto> = self
            .get(
                "preview manual import",
                "manualimport",
                &[("downloadId", download_id)],
            )
            .await?;
        records.into_iter().map(Self::map_preview).collect()
    }

    async fn target(&self, items: &[QueueItem]) -> ArrResult<Target> {
        const OPERATION: &str = "read target";
        if items.is_empty() {
            return Err(ArrRecoveryError::message(OPERATION, "queue group is empty"));
        }
        if self.kind == ArrKind::Radarr {
            let mut ids: Vec<i64> = items.iter().filter_map(|item| item.movie_id).collect();
            ids.sort_unstable();
            ids.dedup();
            let [id] = ids.as_slice() else {
                return Err(ArrRecoveryError::message(
                    OPERATION,
                    "queue group must identify one movie",
                ));
            };
            let movie: MovieDto = self.get(OPERATION, &format!("movie/{id}"), &[]).await?;
            let has_file = movie.has_file();
            return Ok(Target {
                id: movie.id,
                title: movie.title,
                year: movie.year,
                monitored: movie.monitored,
                has_file,
                path: movie.path,
                episode_ids: Vec::new(),
                episodes: Vec::new(),
                alternate_titles: movie
                    .alternate_titles
                    .unwrap_or_default()
                    .into_iter()
                    .map(|t| t.title)
                    .collect(),
            });
        }

        let mut series_ids: Vec<i64> = items.iter().filter_map(|item| item.series_id).collect();
        series_ids.sort_unstable();
        series_ids.dedup();
        let mut episode_ids: Vec<i64> = Vec::new();
        for id in items.iter().filter_map(|item| item.episode_id) {
            if !episode_ids.contains(&id) {
                episode_ids.push(id);
            }
        }
        let ([id], false) = (series_ids.as_slice(), episode_ids.is_empty()) else {
            return Err(ArrRecoveryError::message(
                OPERATION,
                "queue group must identify one series and its episodes",
            ));
        };
        let id = id.to_string();
        let series_path = format!("series/{id}");
        let episode_query = [("seriesId", id.as_str())];
        let (series, all_episodes): (SeriesDto, Vec<EpisodeDto>) = tokio::try_join!(
            self.get(OPERATION, &series_path, &[]),
            self.get(OPERATION, "episode", &episode_query),
        )?;
        let episodes: Vec<EpisodeDto> = all_episodes
            .into_iter()
            .filter(|episode| episode_ids.contains(&episode.id))
            .collect();
        if episodes.len() != episode_ids.len() {
            return Err(ArrRecoveryError::message(
                OPERATION,
                "target episode metadata is incomplete",
            ));
        }
        Ok(Target {
            id: series.id,
            title: series.title,
            year: series.year,
            monitored: series.monitored,
            has_file: episodes.iter().all(|episode| episode.has_file),
            path: series.path,
            episode_ids,
            episodes: episodes
                .into_iter()
                .map(|episode| TargetEpisode {
                    id: episode.id,
                    season_number: episode.season_number,
                    episode_number: episode.episode_number,
                    title: episode.title,
                    has_file: episode.has_file,
                    monitored: episode.monitored,
                })
                .collect(),
            alternate_titles: series
                .alternate_titles
                .unwrap_or_default()
                .into_iter()
                .map(|t| t.title)
                .collect(),
        })
    }

    async fn history(&self, download_id: &str) -> ArrResult<Vec<Grab>> {
        let records: Vec<HistoryDto> = self
            .read_pages(
                "read grab history",
                "history",
                &[("downloadId", download_id), ("eventType", "1")],
            )
            .await?;
        Ok(records
            .into_iter()
            .filter(|record| record.download_id == download_id)
            .map(|record| Grab {
                event_type: value_string(Some(&record.event_type)),
                download_id: record.download_id,
                source_title: record.source_title,
                series_id: record.series_id,
                movie_id: record.movie_id,
                episode_id: record.episode_id,
                date: record.date,
            })
            .collect())
    }

    async fn import_files(&self, download_id: &str, files: &[ImportFile]) -> ArrResult<i64> {
        let sonarr = self.kind == ArrKind::Sonarr;
        let body_files: Vec<Value> = files
            .iter()
            .map(|file| {
                let mut entry = Map::new();
                entry.insert("path".into(), json!(file.path));
                if let Some(folder) = &file.folder_name {
                    entry.insert("folderName".into(), json!(folder));
                }
                if let Some(series_id) = file.series_id {
                    entry.insert("seriesId".into(), json!(series_id));
                }
                if let Some(movie_id) = file.movie_id {
                    entry.insert("movieId".into(), json!(movie_id));
                }
                if sonarr {
                    entry.insert("episodeIds".into(), json!(file.episode_ids));
                }
                entry.insert("quality".into(), Value::Object(file.quality.clone()));
                entry.insert(
                    "languages".into(),
                    json!(file.languages.clone().unwrap_or_default()),
                );
                entry.insert(
                    "releaseGroup".into(),
                    json!(file.release_group.clone().unwrap_or_default()),
                );
                entry.insert(
                    "indexerFlags".into(),
                    json!(file.indexer_flags.unwrap_or(0)),
                );
                if sonarr {
                    entry.insert(
                        "releaseType".into(),
                        json!(file.release_type.as_deref().unwrap_or("unknown")),
                    );
                }
                entry.insert("downloadId".into(), json!(download_id));
                Value::Object(entry)
            })
            .collect();
        let command: CommandDto = self
            .post(
                "start manual import",
                "command",
                json!({ "name": "ManualImport", "importMode": "auto", "files": body_files }),
            )
            .await?;
        Ok(command.id)
    }

    async fn command(&self, id: i64) -> ArrResult<CommandStatus> {
        let command: CommandDto = self
            .get("read command", &format!("command/{id}"), &[])
            .await?;
        Ok(CommandStatus {
            status: value_string(Some(&command.status)),
            message: command.message,
        })
    }

    async fn remove(&self, id: i64, blocklist: bool) -> ArrResult<()> {
        const OPERATION: &str = "remove queue item";
        let url = self.endpoint(
            OPERATION,
            &format!("queue/{id}"),
            &[
                ("removeFromClient", "true"),
                ("blocklist", if blocklist { "true" } else { "false" }),
                ("skipRedownload", "true"),
            ],
        )?;
        self.api
            .void(Method::DELETE, url)
            .await
            .map_err(|failure| op_error(OPERATION, failure))
    }

    async fn verify_removed(&self, output_path: &str) -> ArrResult<bool> {
        if self.local_files {
            filesystem::verify_download_removed(output_path, &filesystem::LocalDirs).await
        } else {
            self.verify_removed_via_api(output_path).await
        }
    }

    async fn search(&self, target: &Target) -> ArrResult<i64> {
        let command = match self.kind {
            ArrKind::Sonarr => json!({ "name": "EpisodeSearch", "episodeIds": target.episode_ids }),
            ArrKind::Radarr => json!({ "name": "MoviesSearch", "movieIds": [target.id] }),
        };
        let result: CommandDto = self
            .post("start replacement search", "command", command)
            .await?;
        Ok(result.id)
    }

    async fn verify_imported(&self, target: &Target, files: &[ImportFile]) -> ArrResult<bool> {
        if files.is_empty() {
            return Ok(false);
        }
        let id = target.id.to_string();
        if self.kind == ArrKind::Radarr {
            const OPERATION: &str = "verify movie import";
            let movie_path = format!("movie/{id}");
            let file_query = [("movieId", id.as_str())];
            let (movie, imported): (MovieDto, Vec<ImportedFileDto>) = tokio::try_join!(
                self.get(OPERATION, &movie_path, &[]),
                self.get(OPERATION, "moviefile", &file_query),
            )?;
            if !movie.has_file() {
                return Ok(false);
            }
            return Ok(files.iter().all(|source| {
                imported.iter().any(|file| {
                    Some(file.id) == movie.movie_file_id
                        && file.movie_id == Some(target.id)
                        && source_matches_file(source, file)
                })
            }));
        }

        const OPERATION: &str = "verify episode import";
        let series_query = [("seriesId", id.as_str())];
        let (episodes, imported): (Vec<EpisodeDto>, Vec<ImportedFileDto>) = tokio::try_join!(
            self.get(OPERATION, "episode", &series_query),
            self.get(OPERATION, "episodefile", &series_query),
        )?;
        let mut wanted = target.episode_ids.clone();
        wanted.sort_unstable();
        wanted.dedup();
        let targeted: Vec<&EpisodeDto> = episodes
            .iter()
            .filter(|episode| wanted.binary_search(&episode.id).is_ok())
            .collect();
        if targeted.len() != wanted.len()
            || targeted
                .iter()
                .any(|episode| !episode.has_file || episode.episode_file_id.unwrap_or(0) == 0)
        {
            return Ok(false);
        }
        if targeted.iter().any(|episode| {
            !imported
                .iter()
                .any(|file| Some(file.id) == episode.episode_file_id)
        }) {
            return Ok(false);
        }
        let covered: Vec<i64> = files
            .iter()
            .flat_map(|file| file.episode_ids.iter().copied())
            .collect();
        if targeted
            .iter()
            .any(|episode| !covered.contains(&episode.id))
        {
            return Ok(false);
        }
        Ok(files.iter().all(|source| {
            if source.episode_ids.is_empty() {
                return false;
            }
            let matching: Vec<i64> = imported
                .iter()
                .filter(|file| source_matches_file(source, file))
                .map(|file| file.id)
                .collect();
            source.episode_ids.iter().all(|episode_id| {
                targeted
                    .iter()
                    .find(|episode| episode.id == *episode_id)
                    .is_some_and(|episode| {
                        episode
                            .episode_file_id
                            .is_some_and(|file_id| matching.contains(&file_id))
                    })
            })
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_video_extensions_case_insensitively() {
        assert_eq!(
            base_name_without_extension("/a/B/Show.S01E01.MKV"),
            "show.s01e01"
        );
        assert_eq!(base_name_without_extension("C:\\dl\\Name.txt"), "name.txt");
        assert!(VIDEO_EXTENSION.is_some());
    }
}
