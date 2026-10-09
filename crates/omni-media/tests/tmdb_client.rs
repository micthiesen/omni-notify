//! TMDB client over a local mock (`src/recommendations/tmdb/client.ts`):
//! key placement, adult filtering, transient retries and typed failures.
#![allow(clippy::expect_used)]

use omni_media::tmdb::{Catalog, TmdbClient};
use omni_media::types::MediaType;
use serde_json::json;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client(server: &MockServer, key: &str) -> TmdbClient {
    TmdbClient::new(
        omni_testkit::mock_http(server, &["https://api.themoviedb.org"]),
        Some(key.to_owned()),
    )
}

#[tokio::test]
async fn sends_v3_keys_in_the_query_and_drops_adult_titles() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/3/search/movie"))
        .and(query_param("query", "dune part two"))
        .and(query_param("include_adult", "false"))
        .and(query_param("year", "2024"))
        .and(query_param("api_key", "v3key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"results": [
            {"id": 1, "title": "Dune: Part Two", "release_date": "2024-02-27", "original_language": "en"},
            {"id": 2, "title": "Adult", "adult": true}
        ]})))
        .mount(&server)
        .await;
    let titles = client(&server, "v3key")
        .search_titles("dune part two", MediaType::Movie, Some(2024))
        .await
        .expect("search");
    assert_eq!(titles.len(), 1);
    assert_eq!(titles[0].year, Some(2024));
}

#[tokio::test]
async fn sends_v4_read_tokens_as_bearer_auth() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/3/movie/603"))
        .and(header("Authorization", "Bearer eyJtoken"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"genres": [{"id": 28, "name": "Action"}]})),
        )
        .mount(&server)
        .await;
    let ids = client(&server, "eyJtoken")
        .title_genre_ids(MediaType::Movie, 603)
        .await
        .expect("genres");
    assert_eq!(ids, vec![28]);
    let requests = server.received_requests().await.expect("requests");
    assert!(
        requests[0]
            .url
            .query()
            .is_none_or(|q| !q.contains("api_key"))
    );
}

#[tokio::test]
async fn retries_transient_failures_then_succeeds() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/3/trending/all/week"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(2)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/3/trending/all/week"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"results": [
            {"media_type": "person", "id": 9, "name": "Someone"},
            {"media_type": "tv", "id": 3, "name": "A Show", "original_language": "en"}
        ]})))
        .mount(&server)
        .await;
    let titles = client(&server, "v3key").trending().await.expect("trending");
    assert_eq!(titles.len(), 1);
    assert_eq!(titles[0].media_type, MediaType::Tv);
    assert_eq!(server.received_requests().await.expect("requests").len(), 3);
}

#[tokio::test]
async fn reports_missing_keys_and_decode_failures_as_integration_errors() {
    let server = MockServer::start().await;
    let unkeyed = TmdbClient::new(omni_testkit::no_network(), None);
    let error = unkeyed.trending().await.expect_err("no key");
    assert_eq!(
        error.to_string(),
        "read TMDB API key failed: TMDB_API_KEY is not configured"
    );
    Mock::given(method("GET"))
        .and(path("/3/genre/tv/list"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"genres": null})))
        .mount(&server)
        .await;
    let error = client(&server, "v3key")
        .genre_map(MediaType::Tv)
        .await
        .expect_err("decode");
    assert_eq!(error.operation, "decode TMDB /genre/tv/list");
    Mock::given(method("GET"))
        .and(path("/3/tv/5"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    let error = client(&server, "v3key")
        .title_details(MediaType::Tv, 5)
        .await
        .expect_err("404");
    assert_eq!(error.operation, "TMDB GET /tv/5");
    assert_eq!(
        server.received_requests().await.expect("requests").len(),
        2,
        "404 is not retried"
    );
}
