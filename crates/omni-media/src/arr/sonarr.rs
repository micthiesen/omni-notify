//! Sonarr series.

use omni_http::SideEffectMode;
use serde::Deserialize;

use super::{ArrConfig, ArrHttp, HttpResult, put_opt};
use crate::js::present;
use crate::types::{AddToWatchlistResult, ExternalIds, MediaItem, MediaType, WatchlistAddOutcome};

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SonarrSeries {
    #[serde(default, deserialize_with = "present")]
    pub id: Option<i64>,
    #[serde(default, deserialize_with = "present")]
    pub title: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub title_slug: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub year: Option<i64>,
    #[serde(default, deserialize_with = "present")]
    pub tvdb_id: Option<i64>,
    #[serde(default, deserialize_with = "present")]
    pub tmdb_id: Option<i64>,
    #[serde(default, deserialize_with = "present")]
    pub imdb_id: Option<String>,
}

/// Tracked series, or `None` when Sonarr is unavailable or unconfigured.
pub async fn fetch_sonarr_series(http: &ArrHttp, config: &ArrConfig) -> Option<Vec<MediaItem>> {
    let connection = config.connection()?;
    match http
        .request_json::<Vec<SonarrSeries>>(connection, "series", None)
        .await
    {
        HttpResult::Ok(series) => Some(series.into_iter().filter_map(normalize).collect()),
        _ => None,
    }
}

fn normalize(series: SonarrSeries) -> Option<MediaItem> {
    let (id, title, tvdb) = (series.id?, series.title?, series.tvdb_id?);
    Some(MediaItem {
        guid: format!("sonarr:{id}"),
        title,
        year: series.year,
        media_type: MediaType::Tv,
        external_ids: Some(ExternalIds {
            tmdb: series.tmdb_id,
            imdb: series.imdb_id,
            tvdb: Some(tvdb),
        }),
        title_slug: series.title_slug,
    })
}

fn find_by(items: &[MediaItem], matches: impl Fn(&ExternalIds) -> bool) -> Option<&MediaItem> {
    items
        .iter()
        .find(|item| item.external_ids.as_ref().is_some_and(&matches))
}

/// `addSonarrSeries`: existing (by TMDB) → lookup → existing (by TVDB) → add
/// (search on) → verify by TVDB. The slug always comes from Sonarr's own list.
pub async fn add_sonarr_series(
    http: &ArrHttp,
    config: &ArrConfig,
    tmdb_id: i64,
) -> WatchlistAddOutcome {
    let (Some(connection), Some((root_folder_path, quality_profile_id))) =
        (config.connection(), config.acquisition())
    else {
        return WatchlistAddOutcome::of(AddToWatchlistResult::Unavailable);
    };
    let Some(existing) = fetch_sonarr_series(http, config).await else {
        return WatchlistAddOutcome::of(AddToWatchlistResult::Unavailable);
    };
    if let Some(tracked) = find_by(&existing, |ids| ids.tmdb == Some(tmdb_id)) {
        return WatchlistAddOutcome {
            result: AddToWatchlistResult::AlreadyExists,
            title_slug: tracked.title_slug.clone(),
        };
    }

    let lookup_path = format!(
        "series/lookup?term={}",
        omni_core::js::encode_uri_component(&format!("tmdb:{tmdb_id}"))
    );
    let lookup = match http
        .request_json::<Vec<SonarrSeries>>(connection, &lookup_path, None)
        .await
    {
        HttpResult::Ok(lookup) => lookup,
        HttpResult::Unavailable => {
            return WatchlistAddOutcome::of(AddToWatchlistResult::Unavailable);
        }
        HttpResult::HttpError { .. } => {
            return WatchlistAddOutcome::of(AddToWatchlistResult::Error);
        }
    };
    let Some(series) = lookup.into_iter().next() else {
        return WatchlistAddOutcome::of(AddToWatchlistResult::NotFound);
    };
    let (Some(title), Some(tvdb_id)) = (
        series.title.clone().filter(|t| !t.is_empty()),
        series.tvdb_id,
    ) else {
        return WatchlistAddOutcome::of(AddToWatchlistResult::NotFound);
    };
    if let Some(tracked) = find_by(&existing, |ids| ids.tvdb == Some(tvdb_id)) {
        return WatchlistAddOutcome {
            result: AddToWatchlistResult::AlreadyExists,
            title_slug: tracked.title_slug.clone(),
        };
    }

    let mut body = serde_json::Map::new();
    body.insert("title".to_owned(), title.into());
    put_opt(&mut body, "titleSlug", series.title_slug.clone());
    put_opt(&mut body, "year", series.year);
    body.insert("tvdbId".to_owned(), tvdb_id.into());
    put_opt(&mut body, "tmdbId", series.tmdb_id);
    put_opt(&mut body, "imdbId", series.imdb_id.clone());
    body.insert("qualityProfileId".to_owned(), quality_profile_id.into());
    body.insert("rootFolderPath".to_owned(), root_folder_path.into());
    body.insert("monitored".to_owned(), true.into());
    body.insert("seasonFolder".to_owned(), true.into());
    body.insert(
        "addOptions".to_owned(),
        serde_json::json!({ "searchForMissingEpisodes": true }),
    );
    let body = serde_json::Value::Object(body);

    if http.mode() == SideEffectMode::Record {
        // Nothing was written, so Sonarr's own list has no slug to report;
        // the lookup's slug is never trusted as the tracked one.
        http.record_write(connection.0, "series", body);
        return WatchlistAddOutcome::of(AddToWatchlistResult::Added);
    }
    match http
        .request_json::<SonarrSeries>(connection, "series", Some(body))
        .await
    {
        HttpResult::Ok(_) => {}
        HttpResult::Unavailable => {
            return WatchlistAddOutcome::of(AddToWatchlistResult::Unavailable);
        }
        HttpResult::HttpError { .. } => {
            return WatchlistAddOutcome::of(AddToWatchlistResult::Error);
        }
    }

    let Some(verified) = fetch_sonarr_series(http, config).await else {
        return WatchlistAddOutcome::of(AddToWatchlistResult::Unavailable);
    };
    match find_by(&verified, |ids| ids.tvdb == Some(tvdb_id)) {
        Some(written) => WatchlistAddOutcome {
            result: AddToWatchlistResult::Added,
            title_slug: written.title_slug.clone(),
        },
        None => WatchlistAddOutcome::of(AddToWatchlistResult::Error),
    }
}
