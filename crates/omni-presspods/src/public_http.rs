//! Bounded public-internet fetches.
//!
//! Every retriever request goes through the SSRF-guarded
//! [`PublicHttpClient`]: the URL, every DNS answer and every redirect hop
//! must be public. Bodies are size-bounded before parsing.
//!
//! TS passed `retry: { limit: 2 }` to these requests, but they ran through
//! `got.stream`, which only retries when the caller listens for its `retry`
//! event; the bounded readers never did, so no PressPods public fetch was
//! ever retried. Every call site therefore uses `retries: 0`; the option stays
//! for callers that genuinely want transient retries (got's 1 s, 2 s backoff).

use std::time::Duration;

use omni_http::public::PublicHttpClient;
use omni_http::{HttpError, Method, Url};

use crate::error::PressPodsError;

pub const PRESS_PODS_HTML_MAX_BYTES: usize = 10 * 1024 * 1024;
pub const PRESS_PODS_JSON_MAX_BYTES: usize = 5 * 1024 * 1024;
pub const PRESS_PODS_IMAGE_MAX_BYTES: usize = 10 * 1024 * 1024;

/// One GET with headers, a whole-request timeout and transient retries.
#[derive(Clone, Debug)]
pub struct PublicGet<'a> {
    pub url: &'a str,
    pub headers: Vec<(&'static str, String)>,
    pub timeout: Duration,
    /// Extra attempts after a transient failure (0 everywhere, see the module docs).
    pub retries: u32,
    pub max_bytes: usize,
    pub operation: &'a str,
}

/// A fetched body with its response headers.
#[derive(Clone, Debug)]
pub struct PublicBody {
    pub body: bytes::Bytes,
    pub headers: omni_http::HeaderMap,
}

fn parse_url(url: &str, operation: &str) -> Result<Url, PressPodsError> {
    Url::parse(url)
        .map_err(|e| PressPodsError::http(operation, HttpError::InvalidUrl(e.to_string())))
}

/// GET returning the bounded body; non-2xx is `HttpError::Status`.
pub async fn fetch_public_buffer(
    client: &PublicHttpClient,
    get: &PublicGet<'_>,
) -> Result<PublicBody, PressPodsError> {
    let url = parse_url(get.url, get.operation)?;
    let mut attempt = 0u32;
    loop {
        let mut request = client
            .request(Method::GET, url.clone())
            .timeout(get.timeout);
        for (name, value) in &get.headers {
            request = request.header(*name, value.as_str());
        }
        let result = request
            .send_bounded(get.max_bytes)
            .await
            .and_then(|response| {
                if response.status.is_success() {
                    Ok(PublicBody {
                        body: response.body,
                        headers: response.headers,
                    })
                } else {
                    Err(HttpError::Status {
                        status: response.status.as_u16(),
                        body: String::from_utf8_lossy(
                            &response.body[..response.body.len().min(4096)],
                        )
                        .into_owned(),
                    })
                }
            });
        match result {
            Ok(body) => return Ok(body),
            Err(error) if error.is_transient() && attempt < get.retries => {
                attempt += 1;
                tokio::time::sleep(Duration::from_millis(1000 << (attempt - 1).min(5))).await;
            }
            Err(error) => return Err(PressPodsError::http(get.operation, error)),
        }
    }
}

/// [`fetch_public_buffer`] decoded as UTF-8 (lossy, like a stream decoder).
pub async fn fetch_public_text(
    client: &PublicHttpClient,
    get: &PublicGet<'_>,
) -> Result<String, PressPodsError> {
    let body = fetch_public_buffer(client, get).await?;
    Ok(String::from_utf8_lossy(&body.body).into_owned())
}

/// [`fetch_public_text`] parsed as JSON (`decode <operation> JSON` on failure).
pub async fn fetch_public_json(
    client: &PublicHttpClient,
    get: &PublicGet<'_>,
) -> Result<serde_json::Value, PressPodsError> {
    let text = fetch_public_text(client, get).await?;
    serde_json::from_str(&text).map_err(|e| {
        PressPodsError::invalid(format!("decode {} JSON", get.operation), e.to_string())
    })
}

/// `fetchPublicHtml`: the retrievers' page fetch.
pub async fn fetch_public_html(
    client: &PublicHttpClient,
    url: &str,
    user_agent: &str,
    max_bytes: usize,
) -> Result<String, PressPodsError> {
    fetch_public_text(
        client,
        &PublicGet {
            url,
            headers: vec![
                ("user-agent", user_agent.to_owned()),
                ("accept", "text/html".to_owned()),
            ],
            timeout: Duration::from_secs(20),
            retries: 0,
            max_bytes,
            operation: "fetch public PressPods HTML",
        },
    )
    .await
}

#[cfg(test)]
mod public_http_spec {
    //! Ports `src/press-pods/publicHttp.spec.ts`. The address and URL rules
    //! are the shared guard in `omni_http::public` (re-exported by TS); the
    //! DNS cases check its answer filter directly because there is no Node
    //! `lookup` callback shape to preserve (the "single shape" and "all-address
    //! shape" cases collapse into one filter returning every answer in order).
    //! Byte-limit cases use a loopback mock server instead of an injected
    //! stream.
    use std::net::IpAddr;

    use omni_http::public::{
        AddressPolicy, assert_public_http_url_syntax, filter_dns_answers, is_public_address,
    };
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn rejects_non_public_address() {
        for address in [
            "127.0.0.1",
            "10.0.0.1",
            "100.64.0.1",
            "169.254.169.254",
            "172.16.0.1",
            "192.168.1.1",
            "224.0.0.1",
            "::1",
            "fc00::1",
            "fe80::1",
            "::ffff:127.0.0.1",
            "::ffff:7f00:1",
            "64:ff9b::7f00:1",
        ] {
            assert!(!is_public_address(ip(address)), "{address}");
        }
    }

    #[test]
    fn allows_public_address() {
        for address in ["1.1.1.1", "8.8.8.8", "2606:4700:4700::1111"] {
            assert!(is_public_address(ip(address)), "{address}");
        }
    }

    #[test]
    fn rejects_unsafe_url_before_any_request() {
        for url in [
            "file:///etc/passwd",
            "ftp://example.com/article",
            "http://localhost/article",
            "http://service.localhost/article",
            "http://127.0.0.1/article",
            "http://[::1]/article",
            "https://user:password@example.com/article",
        ] {
            assert!(assert_public_http_url_syntax(url).is_err(), "{url}");
        }
    }

    #[test]
    fn allows_a_public_http_url() {
        assert_eq!(
            assert_public_http_url_syntax("https://example.com/article")
                .unwrap()
                .as_str(),
            "https://example.com/article"
        );
    }

    #[test]
    fn rejects_mixed_dns_answers_for_a_single_lookup() {
        for answers in [["1.1.1.1", "127.0.0.1"], ["127.0.0.1", "1.1.1.1"]] {
            let result = filter_dns_answers(
                answers.iter().map(|a| ip(a)).collect(),
                AddressPolicy::PublicOnly,
            );
            let error = result.unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("resolve only to public addresses")
            );
        }
    }

    #[test]
    fn validates_every_answer_before_returning_the_requested_single_shape() {
        let answers = vec![ip("1.1.1.1"), ip("2606:4700:4700::1111")];
        let allowed = filter_dns_answers(answers, AddressPolicy::PublicOnly).unwrap();
        assert_eq!(allowed[0], ip("1.1.1.1"));
    }

    #[test]
    fn preserves_the_all_address_lookup_shape_used_by_node_24() {
        let answers = vec![ip("1.1.1.1"), ip("2606:4700:4700::1111")];
        assert_eq!(
            filter_dns_answers(answers.clone(), AddressPolicy::PublicOnly).unwrap(),
            answers
        );
    }

    #[test]
    fn rejects_an_all_address_lookup_containing_any_private_address() {
        let answers = vec![ip("1.1.1.1"), ip("127.0.0.1")];
        assert!(filter_dns_answers(answers, AddressPolicy::PublicOnly).is_err());
    }

    fn client() -> PublicHttpClient {
        PublicHttpClient::new(&omni_testkit::no_network()).allow_loopback_for_tests()
    }

    async fn serve(body: &'static str, chunked: bool) -> MockServer {
        let server = MockServer::start().await;
        let template = if chunked {
            ResponseTemplate::new(200)
                .set_body_raw(body.as_bytes().to_vec(), "text/html")
                .insert_header("transfer-encoding", "chunked")
        } else {
            ResponseTemplate::new(200).set_body_string(body)
        };
        Mock::given(method("GET"))
            .respond_with(template)
            .mount(&server)
            .await;
        server
    }

    fn too_large(error: &PressPodsError, limit: usize) -> bool {
        matches!(error, PressPodsError::Http { source: HttpError::TooLarge { limit: l }, .. } if *l == limit)
    }

    #[tokio::test]
    async fn rejects_an_oversized_fixed_length_html_response() {
        let server = serve("not buffered", false).await;
        let error = fetch_public_html(
            &client(),
            &format!("{}/article", server.uri()),
            "test-agent",
            5,
        )
        .await
        .unwrap_err();
        assert!(too_large(&error, 5), "{error}");
    }

    #[tokio::test]
    async fn rejects_an_oversized_chunked_html_response_while_streaming() {
        let server = serve("123456", true).await;
        let error = fetch_public_html(
            &client(),
            &format!("{}/article", server.uri()),
            "test-agent",
            5,
        )
        .await
        .unwrap_err();
        assert!(too_large(&error, 5), "{error}");
    }

    #[tokio::test]
    async fn bounds_fixed_length_json_before_parsing() {
        let server = serve(r#"{"ok":true}"#, false).await;
        let url = format!("{}/data", server.uri());
        let error = fetch_public_json(
            &client(),
            &PublicGet {
                url: &url,
                headers: vec![],
                timeout: Duration::from_secs(5),
                retries: 0,
                max_bytes: 10,
                operation: "test JSON",
            },
        )
        .await
        .unwrap_err();
        assert!(too_large(&error, 10), "{error}");
    }

    #[tokio::test]
    async fn bounds_chunked_binary_downloads_and_never_returns_a_partial_buffer() {
        let server = serve("\u{1}\u{2}\u{3}\u{4}\u{5}\u{6}", true).await;
        let url = format!("{}/image", server.uri());
        let error = fetch_public_buffer(
            &client(),
            &PublicGet {
                url: &url,
                headers: vec![],
                timeout: Duration::from_secs(5),
                retries: 0,
                max_bytes: 5,
                operation: "test image",
            },
        )
        .await
        .unwrap_err();
        assert!(too_large(&error, 5), "{error}");
    }
}
