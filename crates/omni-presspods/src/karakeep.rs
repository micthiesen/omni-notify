//! Karakeep bookmarking (mitools `Karakeep.addBookmark`), best-effort.
//!
//! Submissions are bookmarked (archived, tagged `PressPods`). The client is
//! disabled without `KARAKEEP_URL` and `KARAKEEP_API_KEY`, and in
//! `SideEffectMode::Record` it records the bookmark instead of creating it.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::BoxFuture;
use omni_config::Config;
use omni_http::{HttpClient, HttpError, Method, SideEffectMode, Url};
use serde::Deserialize;
use serde_json::json;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
/// `HttpClient.retryTransient({ times: 2 })`.
const RETRIES: u32 = 2;

/// Creates bookmarks.
pub trait Bookmarker: Send + Sync {
    /// Returns the bookmark's dashboard URL.
    fn add_bookmark<'a>(
        &'a self,
        url: &'a str,
        tags: &'a [&'a str],
    ) -> BoxFuture<'a, Result<String, KarakeepError>>;
}

#[derive(Debug, thiserror::Error)]
pub enum KarakeepError {
    #[error("Karakeep integration disabled (KARAKEEP_URL / KARAKEEP_API_KEY not set)")]
    Disabled,
    #[error("Karakeep {operation} failed: {message}")]
    Failed {
        operation: &'static str,
        message: String,
    },
}

#[derive(Deserialize)]
struct BookmarkResponse {
    id: String,
}

/// The Karakeep REST client.
pub struct Karakeep {
    http: HttpClient,
    credentials: Option<(String, String)>,
    mode: SideEffectMode,
    recorded: Arc<Mutex<Vec<String>>>,
}

impl Karakeep {
    pub fn new(http: HttpClient, config: &Config, mode: SideEffectMode) -> Self {
        let credentials = match (&config.karakeep_url, &config.karakeep_api_key) {
            (Some(url), Some(key)) if !url.is_empty() && !key.is_empty() => {
                Some((url.trim_end_matches('/').to_owned(), key.clone()))
            }
            _ => None,
        };
        Self {
            http,
            credentials,
            mode,
            recorded: Arc::default(),
        }
    }

    /// URLs captured in record mode.
    pub fn recorded(&self) -> Vec<String> {
        self.recorded.lock().map(|r| r.clone()).unwrap_or_default()
    }

    async fn post(
        &self,
        operation: &'static str,
        base: &str,
        key: &str,
        path: &str,
        body: serde_json::Value,
    ) -> Result<bytes::Bytes, KarakeepError> {
        let failed = |message: String| KarakeepError::Failed { operation, message };
        let url = Url::parse(&format!("{base}/api/v1{path}")).map_err(|e| failed(e.to_string()))?;
        let mut attempt = 0;
        loop {
            let result = self
                .http
                .request(Method::POST, url.clone())
                .bearer_auth(key)
                .json(&body)
                .timeout(REQUEST_TIMEOUT)
                .send_bounded(MAX_RESPONSE_BYTES)
                .await
                .and_then(|response| {
                    if response.status.is_success() {
                        Ok(response.body)
                    } else {
                        Err(HttpError::Status {
                            status: response.status.as_u16(),
                            body: String::from_utf8_lossy(&response.body).into_owned(),
                        })
                    }
                });
            match result {
                Ok(body) => return Ok(body),
                Err(error) if error.is_transient() && attempt < RETRIES => {
                    attempt += 1;
                    tokio::time::sleep(Duration::from_millis(100 << attempt)).await;
                }
                Err(error) => return Err(failed(error.to_string())),
            }
        }
    }
}

impl Bookmarker for Karakeep {
    fn add_bookmark<'a>(
        &'a self,
        url: &'a str,
        tags: &'a [&'a str],
    ) -> BoxFuture<'a, Result<String, KarakeepError>> {
        Box::pin(async move {
            let Some((base, key)) = self.credentials.as_ref() else {
                return Err(KarakeepError::Disabled);
            };
            if self.mode == SideEffectMode::Record {
                if let Ok(mut recorded) = self.recorded.lock() {
                    recorded.push(url.to_owned());
                }
                return Ok(format!("{base}/dashboard/preview/recorded"));
            }
            let created = self
                .post(
                    "createBookmark",
                    base,
                    key,
                    "/bookmarks",
                    json!({ "type": "link", "url": url, "archived": true }),
                )
                .await?;
            let BookmarkResponse { id } =
                serde_json::from_slice(&created).map_err(|e| KarakeepError::Failed {
                    operation: "createBookmark",
                    message: e.to_string(),
                })?;
            if !tags.is_empty() {
                let tags: Vec<serde_json::Value> =
                    tags.iter().map(|t| json!({ "tagName": t })).collect();
                self.post(
                    "attachTags",
                    base,
                    key,
                    &format!("/bookmarks/{id}/tags"),
                    json!({ "tags": tags }),
                )
                .await?;
            }
            Ok(format!("{base}/dashboard/preview/{id}"))
        })
    }
}

#[cfg(test)]
mod tests {
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    fn config(server: &MockServer) -> Config {
        let mut env = omni_testkit::test_app_env();
        env.insert("KARAKEEP_URL".into(), server.uri());
        env.insert("KARAKEEP_API_KEY".into(), "kk-key".into());
        Config::from_env(&env).unwrap()
    }

    #[tokio::test]
    async fn creates_an_archived_bookmark_and_tags_it() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/bookmarks"))
            .and(header("authorization", "Bearer kk-key"))
            .and(body_json(
                json!({"type": "link", "url": "https://a.test/x", "archived": true}),
            ))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id": "b1"})))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/bookmarks/b1/tags"))
            .and(body_json(json!({"tags": [{"tagName": "PressPods"}]})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .expect(1)
            .mount(&server)
            .await;
        let karakeep = Karakeep::new(
            omni_testkit::no_network(),
            &config(&server),
            SideEffectMode::Live,
        );
        let url = karakeep
            .add_bookmark("https://a.test/x", &["PressPods"])
            .await
            .unwrap();
        assert_eq!(url, format!("{}/dashboard/preview/b1", server.uri()));
    }

    #[tokio::test]
    async fn records_instead_of_writing_in_record_mode() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&server)
            .await;
        let karakeep = Karakeep::new(
            omni_testkit::no_network(),
            &config(&server),
            SideEffectMode::Record,
        );
        karakeep
            .add_bookmark("https://a.test/x", &["PressPods"])
            .await
            .unwrap();
        assert_eq!(karakeep.recorded(), ["https://a.test/x"]);
    }

    #[tokio::test]
    async fn is_disabled_without_credentials() {
        let config = Config::from_env(&omni_testkit::test_app_env()).unwrap();
        let karakeep = Karakeep::new(omni_testkit::no_network(), &config, SideEffectMode::Live);
        assert!(matches!(
            karakeep.add_bookmark("https://a.test/x", &[]).await,
            Err(KarakeepError::Disabled)
        ));
    }
}
