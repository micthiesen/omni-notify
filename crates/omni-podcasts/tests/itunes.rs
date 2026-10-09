//! Port of `src/podcast-recs/itunes.spec.ts`. The streamed-response cases run
//! against a local mock server (the TS spec injected a fake stream).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_http::public::PublicHttpClient;
use omni_podcasts::itunes::{ItunesShow, pick_best_show_match, search_itunes_podcasts};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, ResponseTemplate};

fn show(itunes_id: i64, title: &str) -> ItunesShow {
    ItunesShow {
        itunes_id,
        title: title.into(),
        ..ItunesShow::default()
    }
}

#[test]
fn picks_the_exact_normalized_match() {
    let shows = [show(1, "The Daily"), show(2, "Reply All")];
    assert_eq!(pick_best_show_match(&shows, "reply all"), Some(&shows[1]));
}

#[test]
fn matches_despite_punctuation_and_casing_differences() {
    let shows = [show(1, "Radio Lab")];
    assert_eq!(pick_best_show_match(&shows, "radio-lab!!"), Some(&shows[0]));
}

#[test]
fn matches_despite_diacritics() {
    let shows = [show(1, "Café Society")];
    assert_eq!(
        pick_best_show_match(&shows, "cafe society"),
        Some(&shows[0])
    );
}

#[test]
fn falls_back_to_containment_when_the_query_is_a_prefix_of_the_title() {
    let shows = [show(1, "Reply All: The Podcast")];
    assert_eq!(pick_best_show_match(&shows, "Reply All"), Some(&shows[0]));
}

#[test]
fn falls_back_to_containment_when_the_title_is_a_prefix_of_the_query() {
    let shows = [show(1, "Reply All")];
    assert_eq!(
        pick_best_show_match(&shows, "Reply All: The Podcast"),
        Some(&shows[0])
    );
}

#[test]
fn returns_undefined_when_nothing_matches() {
    let shows = [show(1, "The Daily")];
    assert_eq!(
        pick_best_show_match(&shows, "Completely Unrelated Show"),
        None
    );
}

#[test]
fn returns_undefined_for_an_empty_shows_list() {
    assert_eq!(pick_best_show_match(&[], "Anything"), None);
}

async fn client() -> (wiremock::MockServer, PublicHttpClient) {
    let server = omni_testkit::mock_server().await;
    let http = omni_testkit::mock_http(&server, &["https://itunes.apple.com"]);
    (
        server,
        PublicHttpClient::new(&http).allow_loopback_for_tests(),
    )
}

#[tokio::test]
async fn streams_and_decodes_the_bounded_itunes_response() {
    let (server, http) = client().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .and(query_param("media", "podcast"))
        .and(query_param("entity", "podcast"))
        .and(query_param("term", "example"))
        .and(query_param("limit", "5"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"results":[{"collectionId":123,"collectionName":"Example Podcast","feedUrl":"https://example.com/feed.xml","genres":["News"]}]}"#,
        ))
        .expect(1)
        .mount(&server)
        .await;
    let shows = search_itunes_podcasts(&http, "example", 5, 1024)
        .await
        .unwrap();
    assert_eq!(
        shows,
        vec![ItunesShow {
            itunes_id: 123,
            title: "Example Podcast".into(),
            feed_url: Some("https://example.com/feed.xml".into()),
            artwork_url: None,
            genres: vec!["News".into()],
        }]
    );
}

#[tokio::test]
async fn rejects_a_response_that_exceeds_the_byte_limit_while_streaming() {
    let (server, http) = client().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_string("1234567890"))
        .mount(&server)
        .await;
    let error = search_itunes_podcasts(&http, "example", 5, 8)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("8"), "{error}");
}

#[tokio::test]
async fn rejects_a_structurally_invalid_provider_response() {
    let (server, http) = client().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(r#"{"results":[{"collectionId":"123"}]}"#),
        )
        .mount(&server)
        .await;
    assert!(
        search_itunes_podcasts(&http, "example", 5, 1024)
            .await
            .is_err()
    );
}
