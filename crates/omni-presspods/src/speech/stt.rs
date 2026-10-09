//! Speech-to-text for content verification (`src/press-pods/speech/stt.ts`).
//!
//! Points at an OpenAI-compatible `/v1/audio/transcriptions` endpoint, by
//! default the mlx-audio host that serves Higgs. Transcription only has to be
//! good enough to count words (see `coverage`).

use std::time::Duration;

use futures::future::BoxFuture;
use omni_ai::costs::{CostRecorder, NewCostEvent, current_cost_feature};
use omni_api::costs::{CostCategory, CostPriceStatus, CostUsage};
use omni_config::Config;
use omni_http::{HttpClient, HttpError, Method, Url};
use serde::Deserialize;

use crate::error::PressPodsError;
use crate::storage::random_hex;

/// Fast and self-hostable; whisper-large-v3-turbo 500s on that mlx build.
pub const DEFAULT_STT_MODEL: &str = "mlx-community/parakeet-tdt-0.6b-v3";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_ATTEMPTS: u32 = 2;
const STT_ERROR_MAX_BYTES: usize = 64 * 1024;
const STT_JSON_MAX_BYTES: usize = 1024 * 1024;
const OPERATION: &str = "transcribe PressPods chunk";

/// Transcribes MP3 bytes to plain text.
pub trait SttClient: Send + Sync {
    fn model_id(&self) -> &str;
    fn transcribe<'a>(&'a self, mp3: &'a [u8]) -> BoxFuture<'a, Result<String, PressPodsError>>;
}

/// The HTTP client; `None` from [`HttpStt::from_config`] when no endpoint is
/// configured (verification then falls back to the duration band).
pub struct HttpStt {
    http: HttpClient,
    costs: CostRecorder,
    base_url: String,
    model_id: String,
    self_hosted: bool,
}

#[derive(Deserialize)]
struct SttResponse {
    text: Option<String>,
}

/// Retry network failures, 429 and 5xx; never a 4xx.
fn is_transient(status: Option<u16>) -> bool {
    status.is_none_or(|s| s == 429 || s >= 500)
}

/// `multipart/form-data` with text fields and one file part.
fn multipart(fields: &[(&str, &str)], file: (&str, &str, &str, &[u8])) -> (String, Vec<u8>) {
    let boundary = format!("----omni-presspods-{}", random_hex(12));
    let mut body = Vec::new();
    for (name, value) in fields {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    let (name, filename, content_type, bytes) = file;
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\nContent-Type: {content_type}\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

impl HttpStt {
    /// `PRESSPODS_STT_URL`, else the Higgs `PRESSPODS_TTS_URL` (same box).
    pub fn from_config(http: HttpClient, costs: CostRecorder, config: &Config) -> Option<Self> {
        let stt_url = config.presspods_stt_url.clone().filter(|u| !u.is_empty());
        let tts_url = config.presspods_tts_url.clone().filter(|u| !u.is_empty());
        let base_url = stt_url.clone().or_else(|| tts_url.clone())?;
        Some(Self {
            http,
            costs,
            base_url,
            model_id: config
                .presspods_stt_model
                .clone()
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| DEFAULT_STT_MODEL.to_owned()),
            self_hosted: stt_url.is_none() || stt_url == tts_url,
        })
    }

    async fn request(&self, mp3: &[u8]) -> Result<String, PressPodsError> {
        let url = Url::parse(&format!("{}/v1/audio/transcriptions", self.base_url))
            .map_err(|e| PressPodsError::http(OPERATION, HttpError::InvalidUrl(e.to_string())))?;
        let (content_type, body) = multipart(
            &[("model", &self.model_id), ("response_format", "json")],
            ("file", "chunk.mp3", "audio/mpeg", mp3),
        );
        let response = self
            .http
            .request(Method::POST, url)
            .header("content-type", content_type)
            .body(body)
            .timeout(REQUEST_TIMEOUT)
            .send_bounded(STT_JSON_MAX_BYTES.max(STT_ERROR_MAX_BYTES))
            .await
            .map_err(|e| PressPodsError::Failed {
                operation: OPERATION.to_owned(),
                message: e.to_string(),
                retryable: Some(!matches!(
                    e,
                    HttpError::TooLarge { .. } | HttpError::InvalidUrl(_)
                )),
            })?;
        if !response.status.is_success() {
            let status = response.status.as_u16();
            let text = String::from_utf8_lossy(
                &response.body[..response.body.len().min(STT_ERROR_MAX_BYTES)],
            )
            .into_owned();
            let excerpt = omni_core::js::utf16_slice(&text, 0, 200).into_owned();
            return Err(PressPodsError::Failed {
                operation: OPERATION.to_owned(),
                message: format!("STT {status}: {excerpt}"),
                retryable: Some(is_transient(Some(status))),
            });
        }
        let parsed: SttResponse = serde_json::from_slice(&response.body)
            .map_err(|e| PressPodsError::invalid("decode STT response", e.to_string()))?;
        self.costs
            .record(NewCostEvent {
                category: CostCategory::Transcription,
                feature: current_cost_feature("press-pods").to_owned(),
                operation: "verify-audio".to_owned(),
                service: if self.self_hosted {
                    "self-hosted"
                } else {
                    "openai-compatible"
                }
                .to_owned(),
                model: Some(self.model_id.clone()),
                cost_cents: self.self_hosted.then_some(0.0),
                price_status: if self.self_hosted {
                    CostPriceStatus::Free
                } else {
                    CostPriceStatus::Unknown
                },
                usage: CostUsage {
                    requests: Some(1.0),
                    ..CostUsage::default()
                },
                event_id: None,
                incurred_at: None,
                run_id: None,
            })
            .await;
        Ok(parsed.text.unwrap_or_default().trim().to_owned())
    }
}

impl SttClient for HttpStt {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn transcribe<'a>(&'a self, mp3: &'a [u8]) -> BoxFuture<'a, Result<String, PressPodsError>> {
        Box::pin(async move {
            let mut attempt = 1;
            loop {
                match self.request(mp3).await {
                    Ok(text) => return Ok(text),
                    Err(error) => {
                        tracing::debug!(target: "PressPods", "STT request failed ({error})");
                        let retryable = matches!(&error, PressPodsError::Failed { retryable, .. } if *retryable != Some(false));
                        if attempt >= MAX_ATTEMPTS || !retryable {
                            return Err(error);
                        }
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        attempt += 1;
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use wiremock::matchers::{body_string_contains, header_regex, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    async fn client(server: &MockServer) -> (HttpStt, omni_testkit::TestStore) {
        let clock: omni_core::clock::SharedClock =
            omni_testkit::test_clock(omni_testkit::TEST_EPOCH_MS);
        let store = omni_testkit::TestStore::new(clock.clone()).await;
        let mut env = omni_testkit::test_app_env();
        env.insert("PRESSPODS_TTS_URL".into(), server.uri());
        let config = Config::from_env(&env).unwrap();
        let stt = HttpStt::from_config(
            omni_testkit::no_network(),
            CostRecorder::new(store.store.clone(), clock),
            &config,
        )
        .unwrap();
        (stt, store)
    }

    #[tokio::test]
    async fn transcribes_and_records_a_free_self_hosted_event() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .and(header_regex(
                "content-type",
                "^multipart/form-data; boundary=",
            ))
            .and(body_string_contains("mlx-community/parakeet-tdt-0.6b-v3"))
            .and(body_string_contains("filename=\"chunk.mp3\""))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(r#"{"text":"  hello world  "}"#),
            )
            .expect(1)
            .mount(&server)
            .await;
        let (stt, store) = client(&server).await;
        assert_eq!(stt.transcribe(b"mp3").await.unwrap(), "hello world");
        let events = store
            .store
            .read(|docs| omni_store::EntityOps::get_all::<omni_ai::costs::CostEventData>(docs))
            .await
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].service, "self-hosted");
        assert_eq!(events[0].cost_cents, Some(0.0));
    }

    #[tokio::test]
    async fn retries_a_5xx_once_but_never_a_4xx() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503).set_body_string("busy"))
            .expect(2)
            .mount(&server)
            .await;
        let (stt, _store) = client(&server).await;
        let error = stt.transcribe(b"mp3").await.unwrap_err();
        assert!(error.to_string().contains("STT 503: busy"), "{error}");

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(400).set_body_string("bad"))
            .expect(1)
            .mount(&server)
            .await;
        let (stt, _store) = client(&server).await;
        assert!(stt.transcribe(b"mp3").await.is_err());
    }
}
