//! Radarr movies (`src/recommendations/arr/radarr.ts`).

use omni_http::SideEffectMode;
use serde::Deserialize;

use super::{ArrConfig, ArrHttp, HttpResult, put_opt};
use crate::js::present;
use crate::types::{AddToWatchlistResult, ExternalIds, MediaItem, MediaType};

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RadarrMovie {
    #[serde(default, deserialize_with = "present")]
    pub id: Option<i64>,
    #[serde(default, deserialize_with = "present")]
    pub title: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub year: Option<i64>,
    #[serde(default, deserialize_with = "present")]
    pub tmdb_id: Option<i64>,
    #[serde(default, deserialize_with = "present")]
    pub imdb_id: Option<String>,
}

/// Tracked movies, or `None` when Radarr is unavailable or unconfigured.
pub async fn fetch_radarr_movies(http: &ArrHttp, config: &ArrConfig) -> Option<Vec<MediaItem>> {
    let connection = config.connection()?;
    match http
        .request_json::<Vec<RadarrMovie>>(connection, "movie", None)
        .await
    {
        HttpResult::Ok(movies) => Some(movies.into_iter().filter_map(normalize).collect()),
        _ => None,
    }
}

fn normalize(movie: RadarrMovie) -> Option<MediaItem> {
    let (id, title, tmdb) = (movie.id?, movie.title?, movie.tmdb_id?);
    Some(MediaItem {
        guid: format!("radarr:{id}"),
        title,
        year: movie.year,
        media_type: MediaType::Movie,
        external_ids: Some(ExternalIds {
            tmdb: Some(tmdb),
            imdb: movie.imdb_id,
            tvdb: None,
        }),
        title_slug: None,
    })
}

fn tracks(movies: &[MediaItem], tmdb_id: i64) -> bool {
    movies.iter().any(|movie| {
        movie
            .external_ids
            .as_ref()
            .is_some_and(|ids| ids.tmdb == Some(tmdb_id))
    })
}

/// `addRadarrMovie`: existing → lookup → add (search on) → verify by TMDB id.
/// Success is reported only once the movie is visible in Radarr's list.
pub async fn add_radarr_movie(
    http: &ArrHttp,
    config: &ArrConfig,
    tmdb_id: i64,
) -> AddToWatchlistResult {
    let (Some(connection), Some((root_folder_path, quality_profile_id))) =
        (config.connection(), config.acquisition())
    else {
        return AddToWatchlistResult::Unavailable;
    };
    let Some(existing) = fetch_radarr_movies(http, config).await else {
        return AddToWatchlistResult::Unavailable;
    };
    if tracks(&existing, tmdb_id) {
        return AddToWatchlistResult::AlreadyExists;
    }

    let lookup_path = format!(
        "movie/lookup/tmdb?tmdbId={}",
        omni_core::js::encode_uri_component(&tmdb_id.to_string())
    );
    let lookup = match http
        .request_json::<RadarrMovie>(connection, &lookup_path, None)
        .await
    {
        HttpResult::Ok(lookup) => lookup,
        HttpResult::Unavailable => return AddToWatchlistResult::Unavailable,
        HttpResult::HttpError { .. } => return AddToWatchlistResult::Error,
    };
    let (Some(title), Some(lookup_tmdb)) = (
        lookup.title.clone().filter(|t| !t.is_empty()),
        lookup.tmdb_id,
    ) else {
        return AddToWatchlistResult::NotFound;
    };

    let mut body = serde_json::Map::new();
    body.insert("title".to_owned(), title.into());
    put_opt(&mut body, "year", lookup.year);
    body.insert("tmdbId".to_owned(), lookup_tmdb.into());
    put_opt(&mut body, "imdbId", lookup.imdb_id);
    body.insert("qualityProfileId".to_owned(), quality_profile_id.into());
    body.insert("rootFolderPath".to_owned(), root_folder_path.into());
    body.insert("monitored".to_owned(), true.into());
    body.insert(
        "addOptions".to_owned(),
        serde_json::json!({ "searchForMovie": true }),
    );
    let body = serde_json::Value::Object(body);

    if http.mode() == SideEffectMode::Record {
        http.record_write(connection.0, "movie", body);
        return AddToWatchlistResult::Added;
    }
    match http
        .request_json::<RadarrMovie>(connection, "movie", Some(body))
        .await
    {
        HttpResult::Ok(_) => {}
        HttpResult::Unavailable => return AddToWatchlistResult::Unavailable,
        HttpResult::HttpError { .. } => return AddToWatchlistResult::Error,
    }

    match fetch_radarr_movies(http, config).await {
        None => AddToWatchlistResult::Unavailable,
        Some(verified) if tracks(&verified, tmdb_id) => AddToWatchlistResult::Added,
        Some(_) => AddToWatchlistResult::Error,
    }
}
