//! Route behavior for intelligence details and feedback
//! and the `LiveIntelligence` port, over a fake `LiveDirectory`.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::{Arc, OnceLock};

use axum::http::StatusCode;
use futures::future::BoxFuture;
use omni_live_intel::persistence::{NewLivestreamEvent, record_event, save_intelligence};
use omni_live_intel::port::IntelligencePort;
use omni_live_intel::routes::IntelState;
use omni_live_intel::types::{
    EventStatus, LivestreamAlertRecord, LivestreamAlertType, LivestreamEventKind,
    LivestreamIntelligenceData,
};
use omni_runtime::ports::{LiveDirectory, LiveIntelligence as _, PortError};
use omni_testkit::TestApp;
use serde_json::{Value, json};

struct Directory;

impl LiveDirectory for Directory {
    fn streamers(&self) -> BoxFuture<'_, Result<Vec<Value>, PortError>> {
        Box::pin(async {
            Ok(vec![json!({
                "id": "hutch", "displayName": "Hutch", "tier": "background",
                "bindings": [{"platform": "kick", "username": "hutch", "url": "https://kick.com/hutch"}],
            })])
        })
    }
    fn statuses(&self) -> BoxFuture<'_, Result<Vec<Value>, PortError>> {
        Box::pin(async { Ok(vec![]) })
    }
    fn display(&self) -> BoxFuture<'_, Result<Vec<Value>, PortError>> {
        Box::pin(async { Ok(vec![]) })
    }
    fn details<'a>(&'a self, _id: &'a str) -> BoxFuture<'a, Result<Option<Value>, PortError>> {
        Box::pin(async { Ok(None) })
    }
}

const ALERT_ID: &str = "0b9f8c56-1c7c-4a43-9b8e-2f43c1a0d3e1";

async fn app() -> (TestApp, axum::Router) {
    let app = TestApp::new().await;
    app.ctx
        .ports
        .set_live_directory(Arc::new(Directory))
        .expect("directory");
    let subsystem = omni_live_intel::subsystem(&app.ctx);
    for step in subsystem.boot_steps {
        (step.run)(app.ctx.clone()).await.expect("boot step");
    }
    let router = subsystem.router;
    save_intelligence(
        &app.ctx.store,
        LivestreamIntelligenceData {
            streamer_id: "hutch".into(),
            session_started_at: 100,
            relevance_score: 40.0,
            relevance_reasons: vec!["primary channel".into()],
            chapters: vec![],
            updated_at: 200,
            semantic: None,
            trend: None,
            summary: None,
            destiny_presence: None,
            latest_alert: Some(LivestreamAlertRecord {
                alert_id: ALERT_ID.into(),
                alert_type: LivestreamAlertType::Debate,
                title: "Debate".into(),
                message: "Starting".into(),
                reason: "Evidence".into(),
                confidence: 0.9,
                created_at: 150,
                extra: Default::default(),
            }),
            alerted_at_by_type: None,
            extra: Default::default(),
        },
    )
    .await
    .expect("save");
    for index in 0..3_i64 {
        record_event(
            &app.ctx.store,
            NewLivestreamEvent::new(
                "hutch",
                Some(100),
                LivestreamEventKind::Session,
                EventStatus::Info,
                format!("event {index}"),
            )
            .created_at(1_000 + index),
        )
        .await
        .expect("event");
    }
    (app, router)
}

#[tokio::test]
async fn details_return_state_events_and_null_runtime_while_disabled() {
    let (app, router) = app().await;
    let (status, body) = app
        .get_json(&router, "/api/streamers/hutch/intelligence-details?limit=2")
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["intelligence"]["relevanceScore"], json!(40));
    assert_eq!(body["diagnostics"], Value::Null);
    assert_eq!(body["runtime"], Value::Null);
    let events = body["events"].as_array().expect("events");
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["title"], "event 2");
    assert!(body["generatedAt"].is_i64());
    let dto: omni_api::intelligence::IntelligenceDetailsResponse =
        serde_json::from_value(body).expect("matches the omni-api DTO");
    assert_eq!(dto.events.len(), 2);
}

#[tokio::test]
async fn unknown_streamers_are_404() {
    let (app, router) = app().await;
    let (status, body) = app
        .get_json(&router, "/api/streamers/nobody/intelligence-details")
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body, json!({"error": "Unknown streamer"}));
    let (status, body) = app
        .post_json(
            &router,
            "/api/streamers/nobody/intelligence-feedback",
            &json!({"alertId": ALERT_ID, "verdict": "useful"}),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body, json!({"error": "Unknown streamer"}));
}

#[tokio::test]
async fn feedback_is_validated_recorded_and_stale_alerts_are_404() {
    let (app, router) = app().await;
    let path = "/api/streamers/hutch/intelligence-feedback";
    let (status, _) = app
        .post_json(
            &router,
            path,
            &json!({"alertId": "not-a-uuid", "verdict": "useful"}),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, body) = app
        .post_json(
            &router,
            path,
            &json!({"alertId": "11111111-1111-4111-8111-111111111111", "verdict": "useful"}),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body, json!({"error": "Alert no longer exists"}));
    let (status, body) = app
        .post_json(
            &router,
            path,
            &json!({"alertId": ALERT_ID, "verdict": "not_useful", "note": " noisy "}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["feedback"]["feedbackId"], ALERT_ID);
    assert_eq!(body["feedback"]["alertType"], "debate");
    assert_eq!(body["feedback"]["verdict"], "not_useful");
    assert_eq!(body["feedback"]["note"], "noisy");
}

#[tokio::test]
async fn the_port_serves_details_and_feedback() {
    let (app, _router) = app().await;
    // Disabled (the default): no port, so MCP reports the capability as off
    // and the live-check task has no observer.
    assert!(app.ctx.ports.live_intelligence().is_none());
    let port = IntelligencePort::new(IntelState {
        store: app.ctx.store.clone(),
        clock: app.ctx.clock.clone(),
        ports: app.ctx.ports.clone(),
        service: Arc::new(OnceLock::new()),
    });
    let details = port
        .details("hutch", 1)
        .await
        .expect("details")
        .expect("some");
    assert_eq!(details["events"].as_array().map(Vec::len), Some(1));
    assert_eq!(port.diagnostics().await.expect("diagnostics"), Value::Null);
    let feedback = port
        .record_feedback(json!({"streamerId": "hutch", "alertId": ALERT_ID, "verdict": "useful"}))
        .await
        .expect("feedback");
    assert_eq!(feedback["verdict"], "useful");
    // Disabled service: ticks and transitions are no-ops.
    port.after_tick().await;
}

#[tokio::test]
async fn missing_or_corrupt_model_files_disable_intelligence_without_failing_boot() {
    for corrupt in [false, true] {
        let mut app = TestApp::new().await;
        let dir = tempfile::tempdir().expect("tempdir");
        if corrupt {
            let files = omni_live_intel::speech::ModelFiles::in_dir(dir.path());
            for path in files.all() {
                std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
                std::fs::write(path, b"").expect("write empty model");
            }
        }
        let mut config = (*app.ctx.config).clone();
        config.livestream_intelligence_enabled = true;
        config.livestream_model_dir = dir.path().display().to_string();
        app.ctx.config = Arc::new(config);
        let subsystem = omni_live_intel::subsystem(&app.ctx);
        let logs = omni_testkit::capture_logs();
        for step in subsystem.boot_steps {
            (step.run)(app.ctx.clone())
                .await
                .expect("bad models fail closed, not the boot");
        }
        assert!(app.ctx.ports.live_intelligence().is_none());
        let errors: Vec<_> = logs
            .events()
            .into_iter()
            .filter(|e| e.level == tracing::Level::ERROR)
            .collect();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(
            errors[0].message.starts_with(
                "Livestream intelligence disabled: validate livestream speech model files"
            ),
            "{errors:?}"
        );
        assert_eq!(
            errors[0].message.contains("file is empty"),
            corrupt,
            "{errors:?}"
        );
    }
}
