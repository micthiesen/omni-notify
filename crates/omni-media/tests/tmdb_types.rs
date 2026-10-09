//! Port of `src/recommendations/tmdb/types.spec.ts`.
#![allow(clippy::expect_used)]

use omni_media::tmdb::types::{
    FindResponse, MovieDetails, MovieList, TmdbTitle, TmdbTitleDetails, TrendingList, TvDetails,
    TvResult, normalize_movie, normalize_movie_details, normalize_tv, normalize_tv_details,
};
use omni_media::types::MediaType;
use serde_json::json;

#[test]
fn normalizes_a_full_movie_payload() {
    let parsed: MovieList = serde_json::from_value(json!({"results": [{
        "id": 603, "title": "The Matrix", "release_date": "1999-03-30",
        "overview": "A computer hacker...", "genre_ids": [28, 878], "vote_average": 8.2,
        "vote_count": 26000, "popularity": 88.5, "poster_path": "/abc.jpg", "original_language": "en"
    }]}))
    .expect("parse");
    let title = normalize_movie(parsed.results.into_iter().next().expect("result"));
    assert_eq!(
        title,
        TmdbTitle {
            tmdb_id: 603,
            media_type: MediaType::Movie,
            title: "The Matrix".to_owned(),
            year: Some(1999),
            overview: "A computer hacker...".to_owned(),
            genre_ids: vec![28, 878],
            vote_average: 8.2,
            vote_count: 26000.0,
            popularity: 88.5,
            poster_path: Some("/abc.jpg".to_owned()),
            original_language: Some("en".to_owned()),
        }
    );
}

#[test]
fn handles_missing_optional_fields_and_empty_release_dates() {
    let parsed: MovieList = serde_json::from_value(
        json!({"results": [{"id": 1, "title": "Obscure", "release_date": ""}]}),
    )
    .expect("parse");
    let title = normalize_movie(parsed.results.into_iter().next().expect("result"));
    assert_eq!(title.year, None);
    assert_eq!(title.overview, "");
    assert!(title.genre_ids.is_empty());
    assert_eq!(title.poster_path, None);
}

#[test]
fn uses_name_and_first_air_date() {
    let result: TvResult = serde_json::from_value(json!({
        "id": 1396, "name": "Breaking Bad", "first_air_date": "2008-01-20", "overview": "",
        "genre_ids": [18], "vote_average": 8.9, "vote_count": 12000, "popularity": 200,
        "poster_path": null, "original_language": "en", "adult": false
    }))
    .expect("parse");
    let title = normalize_tv(result);
    assert_eq!(title.media_type, MediaType::Tv);
    assert_eq!(title.title, "Breaking Bad");
    assert_eq!(title.year, Some(2008));
    assert_eq!(title.poster_path, None);
    assert_eq!(title.original_language.as_deref(), Some("en"));
}

#[test]
fn tolerates_person_entries_in_trending_results() {
    let parsed: TrendingList = serde_json::from_value(json!({"results": [
        {"media_type": "movie", "id": 1, "title": "A Movie"},
        {"media_type": "person", "id": 2, "name": "An Actor"},
        {"media_type": "tv", "id": 3, "name": "A Show"}
    ]}))
    .expect("parse");
    assert_eq!(parsed.results.len(), 3);
}

#[test]
fn defaults_missing_result_arrays() {
    let parsed: FindResponse = serde_json::from_value(json!({"movie_results": []})).expect("parse");
    assert!(parsed.tv_results.is_empty());
}

#[test]
fn normalizes_movie_commitment_and_creative_metadata() {
    let parsed: MovieDetails = serde_json::from_value(json!({
        "genres": [{"id": 878, "name": "Science Fiction"}],
        "runtime": 136,
        "original_language": "fr",
        "origin_country": ["FR"],
        "credits": {
            "cast": [{"name": "Lead Actor"}, {"name": "Second Actor"}],
            "crew": [{"name": "The Director", "job": "Director"}, {"name": "The Writer", "job": "Writer"}]
        },
        "keywords": {"keywords": [{"name": "time travel"}, {"name": "memory"}]},
        "release_dates": {"results": [{"iso_3166_1": "US", "release_dates": [{"certification": ""}, {"certification": "PG-13"}]}]}
    }))
    .expect("parse");
    assert_eq!(
        normalize_movie_details(parsed),
        TmdbTitleDetails {
            genres: vec!["Science Fiction".to_owned()],
            runtime_minutes: Some(136.0),
            original_language: Some("fr".to_owned()),
            origin_countries: vec!["FR".to_owned()],
            creators: vec!["The Director".to_owned()],
            cast: vec!["Lead Actor".to_owned(), "Second Actor".to_owned()],
            keywords: vec!["time travel".to_owned(), "memory".to_owned()],
            certification: Some("PG-13".to_owned()),
            ..TmdbTitleDetails::default()
        }
    );
}

#[test]
fn normalizes_tv_series_size_and_uses_the_median_episode_runtime() {
    let parsed: TvDetails = serde_json::from_value(json!({
        "genres": [],
        "episode_run_time": [60, 30, 45],
        "number_of_seasons": 3,
        "number_of_episodes": 24,
        "status": "Ended",
        "original_language": "en",
        "origin_country": ["US"],
        "created_by": [{"name": "A Creator"}],
        "credits": {"cast": [{"name": "The Star"}]},
        "keywords": {"results": [{"name": "workplace"}]},
        "content_ratings": {"results": [{"iso_3166_1": "US", "rating": "TV-MA"}]}
    }))
    .expect("parse");
    assert_eq!(
        normalize_tv_details(parsed),
        TmdbTitleDetails {
            genres: vec![],
            runtime_minutes: Some(45.0),
            season_count: Some(3.0),
            episode_count: Some(24.0),
            series_status: Some("Ended".to_owned()),
            original_language: Some("en".to_owned()),
            origin_countries: vec!["US".to_owned()],
            creators: vec!["A Creator".to_owned()],
            cast: vec!["The Star".to_owned()],
            keywords: vec!["workplace".to_owned()],
            certification: Some("TV-MA".to_owned()),
        }
    );
}

#[test]
fn details_serialize_in_ts_key_order_without_absent_fields() {
    let movie = TmdbTitleDetails {
        genres: vec!["Drama".to_owned()],
        runtime_minutes: Some(110.0),
        origin_countries: vec!["US".to_owned()],
        ..TmdbTitleDetails::default()
    };
    let json = omni_core::js::json_stringify(&serde_json::to_value(&movie).expect("json"));
    assert_eq!(
        json,
        r#"{"genres":["Drama"],"runtimeMinutes":110,"originCountries":["US"],"creators":[],"cast":[],"keywords":[]}"#
    );
}

#[test]
fn rejects_null_for_optional_fields() {
    let parsed = serde_json::from_value::<MovieList>(
        json!({"results": [{"id": 1, "title": "X", "overview": null}]}),
    );
    assert!(parsed.is_err());
}
