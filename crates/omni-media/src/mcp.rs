//! Media MCP tools (`src/mcp/tools/media.ts`, `media-shared.ts`).

use std::sync::Arc;

use omni_api::media::{
    MediaType, RecommendationFeedback, RecommendationStatus, TasteProfile, js_number, js_number_opt,
};
use omni_mcp_kit::{McpTool, Page, ToolError, ToolMetaError, paginate, typed_tool};
use serde::{Deserialize, Serialize};

use crate::error::IntegrationError;
use crate::js::js_trim;
use crate::persistence::{
    FeedbackInput, RecommendationData, get_all_recommendations, get_recommendation,
    set_recommendation_feedback,
};
use crate::services::MediaServices;
use crate::taste::{get_all_taste_evidence, get_latest_taste_profile};
use crate::tmdb::types::{TmdbTitle, TmdbTitleDetails};
use crate::tmdb::{DiscoverOptions, tmdb_url};
use crate::types::{ExternalIds, FetchResult, InProgressItem, MediaItem, WatchedItem};
use crate::watchlist::WatchlistAddRequest;

fn default_cursor() -> usize {
    0
}
fn default_catalog_limit() -> usize {
    10
}
fn default_limit() -> usize {
    25
}
fn default_min_votes() -> i64 {
    300
}
fn default_page() -> i64 {
    1
}

fn integration(error: &IntegrationError) -> ToolError {
    ToolError::execute(error.effect_message())
}

fn store_error(error: &omni_store::StoreError) -> ToolError {
    ToolError::execute_from(error)
}

/// `requireAvailable`.
fn require_available<T>(result: FetchResult<T>) -> Result<T, ToolError> {
    match result {
        FetchResult::Ok(value) => Ok(value),
        FetchResult::Unavailable { reason } => Err(ToolError::execute(reason)),
    }
}

/// A trimmed string that must stay non-empty (zod `.trim().min(1)`).
fn non_empty_trimmed(value: &str, field: &str) -> Result<String, ToolError> {
    let trimmed = js_trim(value);
    if trimmed.is_empty() {
        Err(ToolError::input(format!(
            "{field}: Too small: expected string to have >=1 characters"
        )))
    } else {
        Ok(trimmed.to_owned())
    }
}

#[allow(clippy::cast_precision_loss)]
fn num(value: i64) -> f64 {
    value as f64
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CatalogTitle {
    tmdb_id: i64,
    media_type: MediaType,
    title: String,
    year: Option<i64>,
    overview: String,
    genre_ids: Vec<i64>,
    #[serde(serialize_with = "js_number")]
    vote_average: f64,
    #[serde(serialize_with = "js_number")]
    vote_count: f64,
    #[serde(serialize_with = "js_number")]
    popularity: f64,
    poster_path: Option<String>,
    original_language: Option<String>,
    tmdb_url: String,
}

fn serialize_title(title: TmdbTitle) -> CatalogTitle {
    CatalogTitle {
        tmdb_url: tmdb_url(title.media_type, title.tmdb_id),
        tmdb_id: title.tmdb_id,
        media_type: title.media_type,
        title: title.title,
        year: title.year,
        overview: title.overview,
        genre_ids: title.genre_ids,
        vote_average: title.vote_average,
        vote_count: title.vote_count,
        popularity: title.popularity,
        poster_path: title.poster_path,
        original_language: title.original_language,
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolMediaItem {
    guid: String,
    title: String,
    year: Option<i64>,
    media_type: MediaType,
    external_ids: Option<ExternalIds>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "js_number_opt"
    )]
    progress: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_viewed_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    viewed_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    view_count: Option<i64>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "js_number_opt"
    )]
    completion: Option<f64>,
}

impl ToolMediaItem {
    fn base(item: MediaItem) -> Self {
        Self {
            guid: item.guid,
            title: item.title,
            year: item.year,
            media_type: item.media_type,
            external_ids: item.external_ids,
            progress: None,
            last_viewed_at: None,
            viewed_at: None,
            view_count: None,
            completion: None,
        }
    }
    fn watched(item: WatchedItem) -> Self {
        Self {
            viewed_at: Some(item.viewed_at),
            view_count: Some(item.view_count),
            completion: item.completion,
            ..Self::base(item.item)
        }
    }
    fn in_progress(item: InProgressItem) -> Self {
        Self {
            progress: Some(item.progress),
            last_viewed_at: Some(item.last_viewed_at),
            ..Self::base(item.item)
        }
    }
}

/// Media-type and case-insensitive title filters (`toLocaleLowerCase` includes).
fn filter_items(
    items: Vec<ToolMediaItem>,
    media_type: Option<MediaType>,
    query: Option<&str>,
) -> Vec<ToolMediaItem> {
    let needle = query
        .map(js_trim)
        .filter(|q| !q.is_empty())
        .map(str::to_lowercase);
    items
        .into_iter()
        .filter(|item| media_type.is_none_or(|t| item.media_type == t))
        .filter(|item| {
            needle
                .as_deref()
                .is_none_or(|needle| item.title.to_lowercase().contains(needle))
        })
        .collect()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolRecommendation {
    recommendation_id: String,
    canonical_id: String,
    tmdb_id: i64,
    media_type: MediaType,
    title: String,
    year: Option<i64>,
    status: RecommendationStatus,
    why_for_user: Option<String>,
    caveats: Vec<String>,
    #[serde(serialize_with = "js_number_opt")]
    confidence: Option<f64>,
    genres: Vec<String>,
    #[serde(serialize_with = "js_number_opt")]
    runtime_minutes: Option<f64>,
    #[serde(serialize_with = "js_number_opt")]
    season_count: Option<f64>,
    #[serde(serialize_with = "js_number_opt")]
    episode_count: Option<f64>,
    run_date: String,
    recommended_at: i64,
    #[serde(serialize_with = "js_number_opt")]
    notified_at: Option<f64>,
    #[serde(serialize_with = "js_number_opt")]
    resolved_at: Option<f64>,
    watchlist_result: Option<String>,
    feedback: Option<RecommendationFeedback>,
    #[serde(serialize_with = "js_number_opt")]
    feedback_at: Option<f64>,
    feedback_note: Option<String>,
}

fn serialize_recommendation(rec: &RecommendationData) -> ToolRecommendation {
    ToolRecommendation {
        recommendation_id: rec.recommendation_id.clone(),
        canonical_id: rec.canonical_id.clone(),
        tmdb_id: rec.tmdb_id,
        media_type: rec.media_type,
        title: rec.title.clone(),
        year: rec.year,
        status: rec.status(),
        why_for_user: rec.why_for_user.clone(),
        caveats: rec.caveats.clone().unwrap_or_default(),
        confidence: rec.confidence,
        genres: rec.genres.clone().unwrap_or_default(),
        runtime_minutes: rec.runtime_minutes,
        season_count: rec.season_count,
        episode_count: rec.episode_count,
        run_date: rec.run_date.clone(),
        recommended_at: rec.recommended_at,
        notified_at: rec.notified_at.map(num),
        resolved_at: rec.resolved_at.map(num),
        watchlist_result: rec.watchlist_result.map(|w| w.as_str().to_owned()),
        feedback: rec.feedback,
        feedback_at: rec.feedback_at.map(num),
        feedback_note: rec.feedback_note.clone(),
    }
}

#[derive(Serialize)]
struct RecommendationOutput {
    recommendation: ToolRecommendation,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CatalogSearchInput {
    query: String,
    media_type: MediaType,
    year: Option<i64>,
    #[serde(default = "default_cursor")]
    cursor: usize,
    #[serde(default = "default_catalog_limit")]
    limit: usize,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CatalogGetInput {
    media_type: MediaType,
    tmdb_id: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CatalogDetails {
    genres: Vec<String>,
    #[serde(serialize_with = "js_number_opt")]
    runtime_minutes: Option<f64>,
    #[serde(serialize_with = "js_number_opt")]
    season_count: Option<f64>,
    #[serde(serialize_with = "js_number_opt")]
    episode_count: Option<f64>,
    series_status: Option<String>,
    original_language: Option<String>,
    origin_countries: Vec<String>,
    creators: Vec<String>,
    cast: Vec<String>,
    keywords: Vec<String>,
    certification: Option<String>,
}

impl From<TmdbTitleDetails> for CatalogDetails {
    fn from(d: TmdbTitleDetails) -> Self {
        Self {
            genres: d.genres,
            runtime_minutes: d.runtime_minutes,
            season_count: d.season_count,
            episode_count: d.episode_count,
            series_status: d.series_status,
            original_language: d.original_language,
            origin_countries: d.origin_countries,
            creators: d.creators,
            cast: d.cast,
            keywords: d.keywords,
            certification: d.certification,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CatalogGetOutput {
    media_type: MediaType,
    tmdb_id: i64,
    tmdb_url: String,
    details: CatalogDetails,
}

#[derive(Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
enum CatalogBrowseInput {
    Trending {
        #[serde(default = "default_cursor")]
        cursor: usize,
        #[serde(default = "default_catalog_limit")]
        limit: usize,
    },
    Discover {
        #[serde(rename = "mediaType")]
        media_type: MediaType,
        #[serde(rename = "withGenres")]
        with_genres: Option<Vec<i64>>,
        #[serde(rename = "withoutGenres")]
        without_genres: Option<Vec<i64>>,
        #[serde(rename = "originalLanguage")]
        original_language: Option<String>,
        #[serde(rename = "minVoteCount", default = "default_min_votes")]
        min_vote_count: i64,
        #[serde(default = "default_page")]
        page: i64,
        #[serde(default = "default_cursor")]
        cursor: usize,
        #[serde(default = "default_catalog_limit")]
        limit: usize,
    },
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum LibraryView {
    Library,
    History,
    InProgress,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LibraryListInput {
    view: LibraryView,
    media_type: Option<MediaType>,
    query: Option<String>,
    #[serde(default = "default_cursor")]
    cursor: usize,
    #[serde(default = "default_limit")]
    limit: usize,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WatchlistListInput {
    media_type: Option<MediaType>,
    query: Option<String>,
    #[serde(default = "default_cursor")]
    cursor: usize,
    #[serde(default = "default_limit")]
    limit: usize,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WatchlistAddInput {
    tmdb_id: i64,
    media_type: MediaType,
    title: String,
    year: Option<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WatchlistAddOutput {
    result: &'static str,
    title_slug: Option<String>,
}

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum FeedbackFilter {
    GoodPick,
    NotForMe,
    AlreadyWatched,
    None,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecommendationsListInput {
    status: Option<RecommendationStatus>,
    feedback: Option<FeedbackFilter>,
    #[serde(default = "default_cursor")]
    cursor: usize,
    #[serde(default = "default_limit")]
    limit: usize,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecommendationGetInput {
    recommendation_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecommendationFeedbackInput {
    recommendation_id: String,
    feedback: Option<RecommendationFeedback>,
    note: Option<String>,
}

#[derive(Deserialize)]
#[serde(tag = "resource", rename_all = "lowercase")]
enum TasteReadInput {
    Profile {},
    Evidence {
        #[serde(default = "default_cursor")]
        cursor: usize,
        #[serde(default = "default_limit")]
        limit: usize,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EvidenceItem {
    evidence_id: String,
    kind: &'static str,
    canonical_id: String,
    title: String,
    media_type: MediaType,
    observed_at: i64,
    #[serde(serialize_with = "js_number_opt")]
    completion: Option<f64>,
    recommendation_id: Option<String>,
    feedback: Option<RecommendationFeedback>,
    note: Option<String>,
}

#[derive(Serialize)]
#[serde(tag = "resource", rename_all = "lowercase")]
enum TasteReadOutput {
    Profile {
        profile: Option<Box<TasteProfile>>,
    },
    Evidence {
        #[serde(flatten)]
        page: Page<EvidenceItem>,
    },
}

/// The ten media tools in `createMediaTools` order.
pub fn media_tools(services: Arc<MediaServices>) -> Result<Vec<McpTool>, ToolMetaError> {
    let s = services;
    Ok(vec![
        typed_tool("media_catalog_search", {
            let s = s.clone();
            move |input: CatalogSearchInput, _cx| {
                let s = s.clone();
                async move {
                    let query = non_empty_trimmed(&input.query, "query")?;
                    let titles = s
                        .catalog
                        .search_titles(&query, input.media_type, input.year)
                        .await
                        .map_err(|e| integration(&e))?;
                    Ok(paginate(
                        titles.into_iter().map(serialize_title).collect(),
                        input.cursor,
                        input.limit,
                    ))
                }
            }
        })?,
        typed_tool("media_catalog_get", {
            let s = s.clone();
            move |input: CatalogGetInput, _cx| {
                let s = s.clone();
                async move {
                    let details = s
                        .catalog
                        .title_details(input.media_type, input.tmdb_id)
                        .await
                        .map_err(|e| integration(&e))?;
                    Ok(CatalogGetOutput {
                        media_type: input.media_type,
                        tmdb_id: input.tmdb_id,
                        tmdb_url: tmdb_url(input.media_type, input.tmdb_id),
                        details: details.into(),
                    })
                }
            }
        })?,
        typed_tool("media_catalog_browse", {
            let s = s.clone();
            move |input: CatalogBrowseInput, _cx| {
                let s = s.clone();
                async move {
                    let (titles, cursor, limit) = match input {
                        CatalogBrowseInput::Trending { cursor, limit } => {
                            (s.catalog.trending().await, cursor, limit)
                        }
                        CatalogBrowseInput::Discover {
                            media_type,
                            with_genres,
                            without_genres,
                            original_language,
                            min_vote_count,
                            page,
                            cursor,
                            limit,
                        } => {
                            let options = DiscoverOptions {
                                with_genres,
                                without_genres,
                                with_original_language: original_language,
                                min_vote_count: Some(min_vote_count),
                                page: Some(page),
                            };
                            (
                                s.catalog.discover(media_type, &options).await,
                                cursor,
                                limit,
                            )
                        }
                    };
                    let titles = titles.map_err(|e| integration(&e))?;
                    Ok(paginate(
                        titles.into_iter().map(serialize_title).collect(),
                        cursor,
                        limit,
                    ))
                }
            }
        })?,
        typed_tool("media_library_list", {
            let s = s.clone();
            move |input: LibraryListInput, _cx| {
                let s = s.clone();
                async move {
                    let items: Vec<ToolMediaItem> = match input.view {
                        LibraryView::History => require_available(s.library.watch_history().await)?
                            .into_iter()
                            .map(ToolMediaItem::watched)
                            .collect(),
                        LibraryView::InProgress => {
                            require_available(s.library.in_progress().await)?
                                .into_iter()
                                .map(ToolMediaItem::in_progress)
                                .collect()
                        }
                        LibraryView::Library => require_available(s.library.library_index().await)?
                            .into_iter()
                            .map(ToolMediaItem::base)
                            .collect(),
                    };
                    let items = filter_items(items, input.media_type, input.query.as_deref());
                    Ok(paginate(items, input.cursor, input.limit))
                }
            }
        })?,
        typed_tool("media_watchlist_list", {
            let s = s.clone();
            move |input: WatchlistListInput, _cx| {
                let s = s.clone();
                async move {
                    let items = require_available(s.watchlist.fetch().await)?
                        .into_iter()
                        .map(ToolMediaItem::base)
                        .collect();
                    let items = filter_items(items, input.media_type, input.query.as_deref());
                    Ok(paginate(items, input.cursor, input.limit))
                }
            }
        })?,
        typed_tool("media_watchlist_add", {
            let s = s.clone();
            move |input: WatchlistAddInput, _cx| {
                let s = s.clone();
                async move {
                    let title = non_empty_trimmed(&input.title, "title")?;
                    let outcome = s
                        .watchlist
                        .add(&WatchlistAddRequest {
                            tmdb_id: input.tmdb_id,
                            media_type: input.media_type,
                            title,
                            year: input.year,
                            external_ids: None,
                        })
                        .await;
                    Ok(WatchlistAddOutput {
                        result: outcome.result.as_str(),
                        title_slug: outcome.title_slug,
                    })
                }
            }
        })?,
        typed_tool("media_recommendations_list", {
            let s = s.clone();
            move |input: RecommendationsListInput, _cx| {
                let s = s.clone();
                async move {
                    let all = get_all_recommendations(&s.store)
                        .await
                        .map_err(|e| store_error(&e))?;
                    let items = all
                        .iter()
                        .filter(|rec| input.status.is_none_or(|status| rec.status() == status))
                        .filter(|rec| match input.feedback {
                            None => true,
                            Some(FeedbackFilter::None) => rec.feedback.is_none(),
                            Some(FeedbackFilter::GoodPick) => {
                                rec.feedback == Some(RecommendationFeedback::GoodPick)
                            }
                            Some(FeedbackFilter::NotForMe) => {
                                rec.feedback == Some(RecommendationFeedback::NotForMe)
                            }
                            Some(FeedbackFilter::AlreadyWatched) => {
                                rec.feedback == Some(RecommendationFeedback::AlreadyWatched)
                            }
                        })
                        .map(serialize_recommendation)
                        .collect();
                    Ok(paginate(items, input.cursor, input.limit))
                }
            }
        })?,
        typed_tool("media_recommendation_get", {
            let s = s.clone();
            move |input: RecommendationGetInput, _cx| {
                let s = s.clone();
                async move {
                    let rec = get_recommendation(&s.store, &input.recommendation_id)
                        .await
                        .map_err(|e| store_error(&e))?
                        .ok_or_else(|| ToolError::execute("Media recommendation not found"))?;
                    Ok(RecommendationOutput {
                        recommendation: serialize_recommendation(&rec),
                    })
                }
            }
        })?,
        typed_tool("media_recommendation_feedback", {
            let s = s.clone();
            move |input: RecommendationFeedbackInput, _cx| {
                let s = s.clone();
                async move {
                    if input.feedback.is_none() && input.note.is_none() {
                        return Err(ToolError::input("feedback or note is required"));
                    }
                    let rec = set_recommendation_feedback(
                        &s.store,
                        s.now(),
                        &input.recommendation_id,
                        FeedbackInput {
                            feedback: input.feedback,
                            note: input.note.as_deref().map(|n| js_trim(n).to_owned()),
                        },
                    )
                    .await
                    .map_err(|e| store_error(&e))?
                    .ok_or_else(|| ToolError::execute("Media recommendation not found"))?;
                    Ok(RecommendationOutput {
                        recommendation: serialize_recommendation(&rec),
                    })
                }
            }
        })?,
        typed_tool("media_taste_read", {
            let s = s.clone();
            move |input: TasteReadInput, _cx| {
                let s = s.clone();
                async move {
                    match input {
                        TasteReadInput::Profile {} => {
                            let profile = get_latest_taste_profile(&s.store)
                                .await
                                .map_err(|e| store_error(&e))?;
                            Ok(TasteReadOutput::Profile {
                                profile: profile.map(|p| Box::new(p.profile)),
                            })
                        }
                        TasteReadInput::Evidence { cursor, limit } => {
                            let evidence = get_all_taste_evidence(&s.store)
                                .await
                                .map_err(|e| store_error(&e))?;
                            let items = evidence
                                .into_iter()
                                .map(|item| EvidenceItem {
                                    kind: item.kind.as_str(),
                                    evidence_id: item.evidence_id,
                                    canonical_id: item.canonical_id,
                                    title: item.title,
                                    media_type: item.media_type,
                                    observed_at: item.observed_at,
                                    completion: item.completion,
                                    recommendation_id: item.recommendation_id,
                                    feedback: item.feedback,
                                    note: item.note,
                                })
                                .collect();
                            Ok(TasteReadOutput::Evidence {
                                page: paginate(items, cursor, limit),
                            })
                        }
                    }
                }
            }
        })?,
    ])
}
