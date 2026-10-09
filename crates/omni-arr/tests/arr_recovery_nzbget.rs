//! NZBGet health checks against a wiremock server answering successive
//! JSON-RPC calls in order.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Mutex;

use omni_arr::arr_recovery::ArrKind;
use omni_arr::arr_recovery::nzbget::NzbGetClient;
use serde_json::{Value, json};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

struct Sequence(Mutex<Vec<Value>>);

impl Respond for Sequence {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        let mut queue = self.0.lock().unwrap();
        let next = if queue.is_empty() {
            Value::Null
        } else {
            queue.remove(0)
        };
        ResponseTemplate::new(200).set_body_json(json!({ "result": next }))
    }
}

fn item() -> Value {
    json!({
        "NZBID": 29,
        "Status": "WARNING/HEALTH",
        "Category": "sonarr",
        "DestDir": "/tmp/inter/item",
        "FinalDir": "",
        "Parameters": [{ "Name": "drone", "Value": "download" }],
    })
}

async fn health(responses: Vec<Value>) -> Result<Option<String>, String> {
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::path("/jsonrpc"))
        .respond_with(Sequence(Mutex::new(responses)))
        .mount(&server)
        .await;
    let client = NzbGetClient::new(omni_testkit::no_network(), server.uri());
    client
        .download_health(ArrKind::Sonarr, "download", "/tmp/inter/item")
        .await
        .map_err(|e| e.to_string())
}

fn with(changes: Value) -> Value {
    let mut value = item();
    value
        .as_object_mut()
        .unwrap()
        .extend(changes.as_object().unwrap().clone());
    value
}

#[tokio::test]
async fn recognizes_a_parked_health_failure_only_for_the_exact_arr_download_and_path() {
    let result = health(vec![json!([]), json!([item()]), json!([])]).await;
    assert_eq!(result, Ok(Some("WARNING/HEALTH".to_owned())));
}

#[tokio::test]
async fn does_not_touch_an_active_or_restarted_download() {
    assert_eq!(health(vec![json!([item()])]).await, Ok(None));
    assert_eq!(
        health(vec![json!([]), json!([item()]), json!([item()])]).await,
        Ok(None)
    );
}

#[tokio::test]
async fn rejects_another_category_output_path_or_successful_download() {
    for changed in [
        with(json!({ "Category": "radarr" })),
        with(json!({ "DestDir": "/tmp/inter/other" })),
        with(json!({ "Status": "SUCCESS/ALL" })),
    ] {
        assert_eq!(health(vec![json!([]), json!([changed])]).await, Ok(None));
    }
}

#[tokio::test]
async fn fails_closed_for_malformed_rpc_responses() {
    let result = health(vec![json!({})]).await;
    let error = result.unwrap_err();
    assert!(
        error.starts_with("confirm terminal download failure: NZBGet listgroups"),
        "{error}"
    );
}
