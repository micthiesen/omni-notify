//! End to end in `SideEffectMode::Record`: a real HTTP client against a
//! wiremock Radarr. After the 15-minute observation window the exact import
//! is reserved, its submission is recorded instead of sent, the reservation
//! becomes an uncertain outcome, and the Pushover batch is recorded.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use omni_arr::SideEffects;
use omni_arr::arr_recovery::ArrKind;
use omni_arr::arr_recovery::client::{ArrClientConfig, HttpArrClient};
use omni_arr::arr_recovery::persistence::{ActionPhase, NotificationState, RecoveryState};
use omni_arr::arr_recovery::service::{RecoveryContext, STUCK_GRACE_MS, run_recovery};
use omni_arr::arr_recovery::task::{LunaAssessor, PushoverNotifier};
use omni_core::clock::{SharedClock, TestClock};
use omni_http::SideEffectMode;
use omni_store::EntityOps as _;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn mount(server: &MockServer, at: &str, body: serde_json::Value) {
    Mock::given(method("GET"))
        .and(path(at))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

#[tokio::test]
async fn reserves_records_and_reports_without_sending_any_mutation() {
    let server = MockServer::start().await;
    mount(&server, "/api/v3/queue", json!({ "page": 1, "pageSize": 250, "totalRecords": 1, "records": [{
        "id": 1, "downloadId": "download-1", "title": "Example.Movie.2026", "status": "completed",
        "trackedDownloadStatus": "warning", "trackedDownloadState": "importBlocked",
        "statusMessages": [{ "title": "Import failed", "messages": ["Could not find a matching movie"] }],
        "size": 1000, "sizeleft": 0, "outputPath": "/downloads/example", "movieId": 10,
    }]})).await;
    mount(
        &server,
        "/api/v3/movie/10",
        json!({
            "id": 10, "title": "Example Movie", "year": 2026, "monitored": true,
            "path": "/movies/Example Movie", "hasFile": false,
        }),
    )
    .await;
    mount(&server, "/api/v3/manualimport", json!([{
        "id": 20, "path": "/downloads/example/Example.Movie.2026.mkv", "name": "Example.Movie.2026.mkv",
        "size": 1000, "movie": { "id": 10 }, "episodes": [],
        "quality": { "quality": { "id": 7, "name": "Bluray-1080p" }, "revision": { "version": 1, "real": 0 } },
        "rejections": [],
    }])).await;
    mount(
        &server,
        "/api/v3/history",
        json!({ "page": 1, "pageSize": 250, "totalRecords": 1, "records": [{
            "downloadId": "download-1", "sourceTitle": "Example.Movie.2026", "movieId": 10,
            "eventType": "grabbed", "date": "2026-09-12T00:00:00Z",
        }]}),
    )
    .await;

    let app = omni_testkit::TestApp::new().await;
    let side_effects = SideEffects::new(SideEffectMode::Record);
    let client = HttpArrClient::new(ArrClientConfig {
        kind: ArrKind::Radarr,
        url: server.uri(),
        api_key: "key".into(),
        http: omni_testkit::no_network(),
        side_effects: side_effects.clone(),
        local_files: false,
    })
    .unwrap();
    let clock = TestClock::new(1_800_000_000_000);
    let shared: SharedClock = clock.clone();
    let store = omni_testkit::TestStore::new(shared.clone()).await;
    let assessor = LunaAssessor::new(app.ctx.ai.clone(), app.ctx.config.clone());
    let notifier = PushoverNotifier::new(app.ctx.pushover.clone());
    let cx = RecoveryContext {
        store: &store.store,
        clock: &shared,
        assessor: &assessor,
        notifier: &notifier,
        health: None,
        import_settle_delay: Duration::from_millis(10),
    };

    let first = run_recovery(std::slice::from_ref(&client), &cx)
        .await
        .unwrap();
    assert!(first.contains("0 action(s)"), "{first}");
    clock.set(shared.now_ms() + STUCK_GRACE_MS);
    let second = run_recovery(std::slice::from_ref(&client), &cx).await;

    assert!(
        second.is_err(),
        "the recorded submission is not a success: {second:?}"
    );
    let sent = server.received_requests().await.unwrap();
    assert!(
        sent.iter().all(|r| r.method.as_str() == "GET"),
        "no mutation reaches Radarr"
    );
    let recorded = side_effects.recorded();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].body.as_ref().unwrap()["name"], "ManualImport");
    let state = store
        .store
        .read(|docs| docs.get::<RecoveryState>(&"radarr".to_owned()))
        .await
        .unwrap()
        .unwrap();
    let action = &state.actions[0];
    assert_eq!(action.phase, ActionPhase::Uncertain);
    assert_eq!(action.notification, NotificationState::Sent);
    assert!(state.lease.is_none(), "the lease is released");
    let pushes = app.pushes.all();
    assert_eq!(pushes.len(), 1);
    assert_eq!(
        pushes[0].message.title.as_deref(),
        Some("Omni radarr recovery")
    );
    assert!(
        pushes[0]
            .message
            .message
            .starts_with("Needs inspection: Example Movie")
    );
    assert!(
        app.ai.requests().is_empty(),
        "the exact mapping needs no model"
    );
}
