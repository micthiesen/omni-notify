//! TMDB payload schemas and normalization (`src/recommendations/tmdb/types.ts`).
//!
//! Field rules mirror the zod schemas: unknown keys are ignored, `.optional()`
//! accepts an absent key but not `null`, `.nullable()` accepts `null`, and
//! `.default(x)` fills an absent key.

use serde::{Deserialize, Serialize};

use crate::js::present;
use crate::types::MediaType;

/// A normalized TMDB list title (movie or TV).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TmdbTitle {
    pub tmdb_id: i64,
    pub media_type: MediaType,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<i64>,
    pub overview: String,
    pub genre_ids: Vec<i64>,
    pub vote_average: f64,
    pub vote_count: f64,
    pub popularity: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poster_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_language: Option<String>,
}

/// Structured details used to judge fit and viewing commitment. Serialized
/// in the TS object-literal key order with absent fields omitted (taste
/// evidence ids hash `JSON.stringify` of this value).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TmdbTitleDetails {
    pub genres: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_minutes: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub season_count: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode_count: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_language: Option<String>,
    pub origin_countries: Vec<String>,
    pub creators: Vec<String>,
    pub cast: Vec<String>,
    pub keywords: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub certification: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MovieResult {
    pub id: i64,
    pub title: String,
    #[serde(default, deserialize_with = "present")]
    pub release_date: Option<String>,
    #[serde(default)]
    pub overview: String,
    #[serde(default)]
    pub genre_ids: Vec<i64>,
    #[serde(default)]
    pub vote_average: f64,
    #[serde(default)]
    pub vote_count: f64,
    #[serde(default)]
    pub popularity: f64,
    #[serde(default)]
    pub poster_path: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub original_language: Option<String>,
    #[serde(default)]
    pub adult: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct TvResult {
    pub id: i64,
    pub name: String,
    #[serde(default, deserialize_with = "present")]
    pub first_air_date: Option<String>,
    #[serde(default)]
    pub overview: String,
    #[serde(default)]
    pub genre_ids: Vec<i64>,
    #[serde(default)]
    pub vote_average: f64,
    #[serde(default)]
    pub vote_count: f64,
    #[serde(default)]
    pub popularity: f64,
    #[serde(default)]
    pub poster_path: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub original_language: Option<String>,
    #[serde(default)]
    pub adult: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MovieList {
    pub results: Vec<MovieResult>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct TvList {
    pub results: Vec<TvResult>,
}

/// One trending entry: a movie, a TV show, or anything else (people) that
/// carries a string `media_type` (zod `union([movie, tv, other])`).
#[derive(Clone, Debug)]
pub enum TrendingResult {
    Movie(MovieResult),
    Tv(TvResult),
    Other,
}

impl<'de> Deserialize<'de> for TrendingResult {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        let media_type = value
            .get("media_type")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        // A union member parses only when the literal matches and its object parses.
        if media_type.as_deref() == Some("movie")
            && let Ok(movie) = serde_json::from_value::<MovieResult>(value.clone())
        {
            return Ok(TrendingResult::Movie(movie));
        }
        if media_type.as_deref() == Some("tv")
            && let Ok(tv) = serde_json::from_value::<TvResult>(value.clone())
        {
            return Ok(TrendingResult::Tv(tv));
        }
        match media_type {
            Some(_) => Ok(TrendingResult::Other),
            None => Err(serde::de::Error::custom(
                "trending result has no string media_type",
            )),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct TrendingList {
    pub results: Vec<TrendingResult>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct FindResponse {
    #[serde(default)]
    pub movie_results: Vec<MovieResult>,
    #[serde(default)]
    pub tv_results: Vec<TvResult>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Genre {
    pub id: i64,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Details {
    pub genres: Vec<Genre>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct NamedPerson {
    pub name: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CrewMember {
    pub name: String,
    #[serde(default, deserialize_with = "present")]
    pub job: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MovieCredits {
    #[serde(default)]
    pub cast: Vec<NamedPerson>,
    #[serde(default)]
    pub crew: Vec<CrewMember>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MovieKeywords {
    #[serde(default)]
    pub keywords: Vec<NamedPerson>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Certification {
    pub certification: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CountryReleases {
    pub iso_3166_1: String,
    #[serde(default)]
    pub release_dates: Vec<Certification>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ReleaseDates {
    #[serde(default)]
    pub results: Vec<CountryReleases>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MovieDetails {
    pub genres: Vec<Genre>,
    #[serde(default)]
    pub runtime: Option<f64>,
    #[serde(default, deserialize_with = "present")]
    pub original_language: Option<String>,
    #[serde(default)]
    pub origin_country: Vec<String>,
    #[serde(default, deserialize_with = "present")]
    pub credits: Option<MovieCredits>,
    #[serde(default, deserialize_with = "present")]
    pub keywords: Option<MovieKeywords>,
    #[serde(default, deserialize_with = "present")]
    pub release_dates: Option<ReleaseDates>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct TvCredits {
    #[serde(default)]
    pub cast: Vec<NamedPerson>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct TvKeywords {
    #[serde(default)]
    pub results: Vec<NamedPerson>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ContentRating {
    pub iso_3166_1: String,
    pub rating: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ContentRatings {
    #[serde(default)]
    pub results: Vec<ContentRating>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct TvDetails {
    pub genres: Vec<Genre>,
    #[serde(default)]
    pub episode_run_time: Vec<f64>,
    #[serde(default, deserialize_with = "present")]
    pub number_of_seasons: Option<f64>,
    #[serde(default, deserialize_with = "present")]
    pub number_of_episodes: Option<f64>,
    #[serde(default, deserialize_with = "present")]
    pub status: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub original_language: Option<String>,
    #[serde(default)]
    pub origin_country: Vec<String>,
    #[serde(default)]
    pub created_by: Vec<NamedPerson>,
    #[serde(default, deserialize_with = "present")]
    pub credits: Option<TvCredits>,
    #[serde(default, deserialize_with = "present")]
    pub keywords: Option<TvKeywords>,
    #[serde(default, deserialize_with = "present")]
    pub content_ratings: Option<ContentRatings>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct GenreList {
    pub genres: Vec<Genre>,
}

/// `parseYear`: the leading four digits when they form a year after 1800.
fn parse_year(date: Option<&str>) -> Option<i64> {
    let date = date.filter(|d| !d.is_empty())?;
    let prefix = omni_core::js::utf16_slice(date, 0, 4);
    let year = omni_core::js::string_to_number(&prefix);
    #[allow(clippy::cast_possible_truncation)]
    (year.is_finite() && year > 1800.0).then_some(year as i64)
}

/// Trimmed, or `None` when empty.
pub fn non_empty(value: Option<&str>) -> Option<String> {
    let trimmed = crate::js::js_trim(value?);
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

pub fn normalize_movie(result: MovieResult) -> TmdbTitle {
    TmdbTitle {
        tmdb_id: result.id,
        media_type: MediaType::Movie,
        year: parse_year(result.release_date.as_deref()),
        title: result.title,
        overview: result.overview,
        genre_ids: result.genre_ids,
        vote_average: result.vote_average,
        vote_count: result.vote_count,
        popularity: result.popularity,
        poster_path: result.poster_path,
        original_language: non_empty(result.original_language.as_deref()),
    }
}

pub fn normalize_tv(result: TvResult) -> TmdbTitle {
    TmdbTitle {
        tmdb_id: result.id,
        media_type: MediaType::Tv,
        year: parse_year(result.first_air_date.as_deref()),
        title: result.name,
        overview: result.overview,
        genre_ids: result.genre_ids,
        vote_average: result.vote_average,
        vote_count: result.vote_count,
        popularity: result.popularity,
        poster_path: result.poster_path,
        original_language: non_empty(result.original_language.as_deref()),
    }
}

fn names(people: &[NamedPerson], limit: usize) -> Vec<String> {
    people.iter().take(limit).map(|p| p.name.clone()).collect()
}

pub fn normalize_movie_details(result: MovieDetails) -> TmdbTitleDetails {
    let us_releases = result.release_dates.as_ref().and_then(|dates| {
        dates
            .results
            .iter()
            .find(|entry| entry.iso_3166_1 == "US")
            .map(|entry| &entry.release_dates)
    });
    let certification = us_releases.and_then(|releases| {
        releases
            .iter()
            .find(|release| non_empty(Some(&release.certification)).is_some())
            .and_then(|release| non_empty(Some(&release.certification)))
    });
    TmdbTitleDetails {
        genres: result.genres.iter().map(|g| g.name.clone()).collect(),
        runtime_minutes: result.runtime.filter(|r| *r > 0.0),
        season_count: None,
        episode_count: None,
        series_status: None,
        original_language: non_empty(result.original_language.as_deref()),
        origin_countries: result.origin_country,
        creators: result
            .credits
            .as_ref()
            .map(|credits| {
                credits
                    .crew
                    .iter()
                    .filter(|person| person.job.as_deref() == Some("Director"))
                    .take(3)
                    .map(|person| person.name.clone())
                    .collect()
            })
            .unwrap_or_default(),
        cast: result
            .credits
            .as_ref()
            .map(|credits| names(&credits.cast, 6))
            .unwrap_or_default(),
        keywords: result
            .keywords
            .as_ref()
            .map(|k| names(&k.keywords, 12))
            .unwrap_or_default(),
        certification,
    }
}

pub fn normalize_tv_details(result: TvDetails) -> TmdbTitleDetails {
    let mut runtimes: Vec<f64> = result
        .episode_run_time
        .iter()
        .copied()
        .filter(|r| *r > 0.0)
        .collect();
    runtimes.sort_by(f64::total_cmp);
    let typical = runtimes.get(runtimes.len() / 2).copied();
    TmdbTitleDetails {
        genres: result.genres.iter().map(|g| g.name.clone()).collect(),
        runtime_minutes: typical,
        season_count: result.number_of_seasons,
        episode_count: result.number_of_episodes,
        series_status: non_empty(result.status.as_deref()),
        original_language: non_empty(result.original_language.as_deref()),
        origin_countries: result.origin_country,
        creators: names(&result.created_by, 3),
        cast: result
            .credits
            .as_ref()
            .map(|c| names(&c.cast, 6))
            .unwrap_or_default(),
        keywords: result
            .keywords
            .as_ref()
            .map(|k| names(&k.results, 12))
            .unwrap_or_default(),
        certification: non_empty(
            result
                .content_ratings
                .as_ref()
                .and_then(|r| r.results.iter().find(|e| e.iso_3166_1 == "US"))
                .map(|e| e.rating.as_str()),
        ),
    }
}
