//! The SSRF-guarded public HTTP client: address checks, redirects and response
//! size limits.
//!
//! Cancellation is dropping the request future; `times_out_across_the_whole_request`
//! in `http_client.rs` covers bounded waiting. hyper delivers headers before any
//! body read, so the Content-Length precheck always runs first
//! (`rejects_an_oversized_fixed_length_response_before_buffering_it`). The
//! chunked overflow cases live in `http_client.rs`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::IpAddr;

use omni_http::public::{
    AddressPolicy, PublicHttpClient, assert_public_http_url_syntax, filter_dns_answers,
    is_public_address,
};
use omni_http::{HttpClient, HttpConfig, HttpError, Method, Url};
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn ip(s: &str) -> IpAddr {
    s.parse().unwrap()
}

#[test]
fn rejects_private_loopback_link_local_and_mapped_addresses() {
    for address in [
        "127.0.0.1",
        "10.1.2.3",
        "169.254.169.254",
        "192.168.1.2",
        "::1",
        "fe80::1",
        "::ffff:127.0.0.1",
        "192.88.99.1",
        "2001:20::1",
        "fec0::1",
        "4000::1",
    ] {
        assert!(!is_public_address(ip(address)), "{address}");
    }
    assert!(is_public_address(ip("1.1.1.1")));
    assert!(is_public_address(ip("2606:4700:4700::1111")));
}

#[test]
fn rejects_private_redirect_targets_before_a_connection() {
    for target in [
        "http://127.0.0.1/admin",
        "http://169.254.169.254/latest/meta-data",
    ] {
        let error = assert_public_http_url_syntax(target).unwrap_err();
        assert!(error.to_string().contains("public host"), "{error}");
    }
}

#[test]
fn rejects_mixed_dns_answers_in_either_order() {
    for answers in [["1.1.1.1", "127.0.0.1"], ["127.0.0.1", "1.1.1.1"]] {
        let error = filter_dns_answers(
            answers.iter().map(|a| ip(a)).collect(),
            AddressPolicy::PublicOnly,
        )
        .unwrap_err();
        assert!(error.to_string().contains("public addresses"));
    }
    assert!(filter_dns_answers(Vec::new(), AddressPolicy::PublicOnly).is_err());
}

#[test]
fn validates_every_answer_and_keeps_resolver_order() {
    let answers = vec![ip("1.1.1.1"), ip("2606:4700:4700::1111")];
    assert_eq!(
        filter_dns_answers(answers.clone(), AddressPolicy::PublicOnly).unwrap(),
        answers
    );
}

fn public_client() -> PublicHttpClient {
    let base = HttpClient::new(HttpConfig::default()).unwrap();
    PublicHttpClient::new(&base).allow_loopback_for_tests()
}

#[tokio::test]
async fn production_policy_blocks_loopback_before_connecting() {
    let base = HttpClient::new(HttpConfig::default()).unwrap();
    let result = PublicHttpClient::new(&base)
        .request(Method::GET, Url::parse("http://127.0.0.1:9/").unwrap())
        .send_bounded(16)
        .await;
    assert!(matches!(result, Err(HttpError::Blocked(_))));
}

#[tokio::test]
async fn revalidates_every_redirect_hop() {
    let server = MockServer::start().await;
    Mock::given(path("/private"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", "http://10.0.0.1/admin"))
        .mount(&server)
        .await;
    Mock::given(path("/metadata"))
        .respond_with(
            ResponseTemplate::new(301)
                .insert_header("location", "http://169.254.169.254/latest/meta-data"),
        )
        .mount(&server)
        .await;
    Mock::given(path("/hop"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", "/final"))
        .mount(&server)
        .await;
    Mock::given(path("/final"))
        .respond_with(ResponseTemplate::new(200).set_body_string("public"))
        .mount(&server)
        .await;

    for blocked in ["/private", "/metadata"] {
        let result = public_client()
            .request(
                Method::GET,
                Url::parse(&format!("{}{blocked}", server.uri())).unwrap(),
            )
            .send_bounded(1024)
            .await;
        assert!(
            matches!(result, Err(HttpError::Blocked(_))),
            "{blocked}: {result:?}"
        );
    }
    let followed = public_client()
        .request(
            Method::GET,
            Url::parse(&format!("{}/hop", server.uri())).unwrap(),
        )
        .send_bounded(1024)
        .await
        .unwrap();
    assert_eq!(&followed.body[..], b"public");
}
