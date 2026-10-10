//! Streamer routes, the `LiveDirectory` port
//! and subsystem assembly.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::Write;

use omni_api::streamers::{LivestreamDetails, LivestreamSummary, StreamerView};
use omni_live::channels::parse_channels_config;
use omni_live::sessions::{StreamSession, StreamSessions};
use omni_live::status::{LiveSource, LiveStatus, OfflineStatus, StreamerStatus, upsert_status};
use omni_live::{LiveModule, Platform, PlatformBinding, Roster, StreamerConfigService};
use omni_runtime::ports::LiveDirectory;
use omni_store::cbor::{Extra, JsValue};
use omni_store::entity::{EntityWrite, UpsertOpts};
use omni_store::{DocMeta, DocWrite};
use serde_json::json;

const CHANNELS: &str = r#"{
  "Destiny": {"youtube": "@destiny", "kick": "destiny"},
  "Jerma": {"twitch": "jerma985", "tier": "background"},
  "Hutch": {"youtube": "@hutch"}
}"#;

async fn app() -> (omni_testkit::TestApp, axum::Router, LiveModule) {
    let app = omni_testkit::TestApp::new().await;
    let module =
        LiveModule::from_config(&app.ctx, parse_channels_config(CHANNELS).unwrap()).unwrap();
    let again =
        LiveModule::from_config(&app.ctx, parse_channels_config(CHANNELS).unwrap()).unwrap();
    let subsystem = module.into_subsystem(None).unwrap();
    let router = app.router(&subsystem);
    (app, router, again)
}

async fn seed(app: &omni_testkit::TestApp) {
    let store = &app.ctx.store;
    upsert_status(
        store,
        StreamerStatus::Live(LiveStatus {
            streamer_id: "jerma".into(),
            primary: PlatformBinding::new(Platform::Twitch, "jerma985"),
            primary_title: "Jerma title".into(),
            started_at: 1_000,
            max_viewer_count: 20_000,
            viewer_count: Some(15_000),
            sources: Some(vec![LiveSource {
                platform: Platform::Twitch,
                username: "jerma985".into(),
                title: "Jerma title".into(),
                viewer_count: Some(15_000),
                category: Some("Games".into()),
            }]),
            category: Some("Games".into()),
            extra: Extra::new(),
        }),
    )
    .await
    .unwrap();
    upsert_status(
        store,
        StreamerStatus::Live(LiveStatus {
            streamer_id: "destiny".into(),
            primary: PlatformBinding::new(Platform::Kick, "destiny"),
            primary_title: "Destiny title".into(),
            started_at: 2_000,
            max_viewer_count: 30,
            viewer_count: None,
            sources: None,
            category: None,
            extra: Extra::new(),
        }),
    )
    .await
    .unwrap();
    let mut offline = OfflineStatus::never_live("hutch");
    offline.last_ended_at = Some(5_000);
    offline.last_started_at = Some(4_000);
    offline.last_max_viewer_count = Some(12);
    upsert_status(store, StreamerStatus::Offline(offline))
        .await
        .unwrap();
    // A raw intelligence document with a Date and an undefined member.
    let doc = JsValue::Object(
        [
            ("streamerId".to_owned(), JsValue::String("destiny".into())),
            ("sessionStartedAt".to_owned(), JsValue::Int(2_000)),
            ("relevanceScore".to_owned(), JsValue::Float(0.5)),
            ("dropped".to_owned(), JsValue::Undefined),
            ("at".to_owned(), JsValue::Date(0.0)),
        ]
        .into_iter()
        .collect(),
    );
    store
        .write(move |tx| {
            tx.upsert_doc(
                "$livestream-intelligence#s7:destiny",
                &doc,
                DocMeta {
                    entity: Some("livestream-intelligence".into()),
                    version: 0,
                    expires_at: None,
                    updated_at: None,
                },
            )
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn lists_live_streamers_first_by_tier_then_offline_by_recency() {
    let (app, router, _) = app().await;
    seed(&app).await;
    let (status, body) = app.get_json(&router, "/api/streamers").await;
    assert_eq!(status, 200);
    let ids: Vec<&str> = body["streamers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["destiny", "jerma", "hutch"]);
    assert_eq!(
        body["streamers"][0],
        json!({
            "id": "destiny", "displayName": "Destiny",
            // TestApp has no Kick credentials, so the Kick binding was dropped at boot.
            "bindings": [
                {"platform": "youtube", "username": "@destiny", "url": "https://www.youtube.com/@destiny/live"}
            ],
            "tier": "primary", "live": true, "title": "Destiny title", "startedAt": 2000,
            "maxViewerCount": 30, "viewerCount": null, "sources": [], "category": null,
            "primary": {"platform": "kick", "username": "destiny", "url": "https://kick.com/destiny"},
            "intelligence": {"streamerId": "destiny", "sessionStartedAt": 2000, "relevanceScore": 0.5, "at": "1970-01-01T00:00:00.000Z"}
        })
    );
    assert_eq!(
        body["streamers"][1]["sources"],
        json!([{"platform": "twitch", "username": "jerma985", "title": "Jerma title", "viewerCount": 15000, "category": "Games"}])
    );
    assert_eq!(
        body["streamers"][2],
        json!({
            "id": "hutch", "displayName": "Hutch",
            "bindings": [{"platform": "youtube", "username": "@hutch", "url": "https://www.youtube.com/@hutch/live"}],
            "tier": "primary", "live": false, "lastStartedAt": 4000, "lastEndedAt": 5000, "lastMaxViewerCount": 12
        })
    );
    let parsed: Vec<StreamerView> = serde_json::from_value(body["streamers"].clone()).unwrap();
    assert_eq!(parsed.len(), 3);
}

#[tokio::test]
async fn returns_trigger_channels() {
    let (app, router, _) = app().await;
    let (status, body) = app.get_json(&router, "/api/trigger-channels").await;
    assert_eq!(status, 200);
    assert_eq!(
        body,
        json!({"channels": [
            {"key": "destiny", "displayName": "Destiny", "type": "youtube", "url": "https://www.youtube.com/@destiny/live"},
            {"key": "jerma", "displayName": "Jerma", "type": "twitch"},
            {"key": "hutch", "displayName": "Hutch", "type": "youtube", "url": "https://www.youtube.com/@hutch/live"}
        ]})
    );
}

#[tokio::test]
async fn serves_metrics_and_newest_first_sessions_for_known_streamers_only() {
    let (app, router, _) = app().await;
    let sessions = StreamSessions {
        streamer_id: "jerma".into(),
        sessions: (1..=2)
            .map(|i| StreamSession {
                started_at: i * 100,
                ended_at: i * 100 + 50,
                duration_ms: 50,
                peak_viewers: i,
                title: format!("s{i}"),
                platform: Platform::Twitch,
                username: "jerma985".into(),
            })
            .collect(),
        extra: Extra::new(),
    };
    app.ctx
        .store
        .write(move |tx| tx.upsert(&sessions, UpsertOpts::default()))
        .await
        .unwrap();

    let (status, body) = app.get_json(&router, "/api/streamers/jerma/sessions").await;
    assert_eq!(status, 200);
    let titles: Vec<&str> = body["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, ["s2", "s1"]);

    let (status, body) = app.get_json(&router, "/api/streamers/jerma/metrics").await;
    assert_eq!(status, 200);
    assert_eq!(
        body,
        json!({"dailyBuckets": [], "allTimeMax": 0, "allTimeMaxTimestamp": 0, "platforms": []})
    );

    for path in [
        "/api/streamers/nobody/metrics",
        "/api/streamers/nobody/sessions",
    ] {
        let (status, body) = app.get_json(&router, path).await;
        assert_eq!(status, 404);
        assert_eq!(body, json!({"error": "Unknown streamer"}));
    }
}

#[tokio::test]
async fn implements_the_live_directory_port() {
    let (app, _router, module) = app().await;
    seed(&app).await;
    let directory = app
        .ctx
        .ports
        .live_directory()
        .expect("port set by into_subsystem");
    let summaries: Vec<LivestreamSummary> = serde_json::from_value(serde_json::Value::Array(
        directory.streamers().await.unwrap(),
    ))
    .unwrap();
    assert_eq!(
        summaries.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
        ["destiny", "jerma", "hutch"]
    );
    assert_eq!(summaries[2].max_viewer_count, Some(12));
    assert!(summaries[1].live);
    let statuses = directory.statuses().await.unwrap();
    assert_eq!(statuses[0]["isLive"], json!(true));
    assert_eq!(directory.display().await.unwrap().len(), 3);
    let details: LivestreamDetails =
        serde_json::from_value(directory.details("jerma").await.unwrap().unwrap()).unwrap();
    assert_eq!(details.livestream.title.as_deref(), Some("Jerma title"));
    assert!(directory.details("nobody").await.unwrap().is_none());
    // The module's own handle reads the same roster.
    assert_eq!(module.directory().streamers().await.unwrap().len(), 3);
}

#[tokio::test]
async fn registers_the_task_and_drops_kick_without_credentials() {
    let app = omni_testkit::TestApp::new().await;
    let module =
        LiveModule::from_config(&app.ctx, parse_channels_config(CHANNELS).unwrap()).unwrap();
    // TestApp has no Kick credentials: Destiny keeps only its YouTube binding.
    let destiny = module.roster().get("destiny").unwrap();
    assert_eq!(
        destiny.bindings,
        vec![PlatformBinding::new(Platform::YouTube, "@destiny")]
    );
    let subsystem = module.into_subsystem(None).unwrap();
    assert_eq!(subsystem.tasks.len(), 1);
    assert_eq!(subsystem.tasks[0].name(), "LiveCheckTask");
    assert_eq!(subsystem.tasks[0].schedule().as_str(), "*/20 * * * * *");
    assert_eq!(
        subsystem.tasks[0].options().jitter,
        std::time::Duration::from_secs(3)
    );
    assert!(subsystem.tasks[0].options().run_on_startup);
    assert_eq!(subsystem.entities.len(), 7);
    assert_eq!(subsystem.mcp_tools.len(), 6);
    // An in-memory configuration has no boot step.
    assert!(subsystem.boot_steps.is_empty());

    // Streamers can be added at runtime, so the task always runs.
    let empty = LiveModule::load(&app.ctx)
        .unwrap()
        .into_subsystem(None)
        .unwrap();
    assert_eq!(empty.tasks.len(), 1);
    assert_eq!(empty.boot_steps.len(), 1);
}

/// Runs the production boot step; returns the roster and config service.
async fn boot(app: &omni_testkit::TestApp) -> Result<(Roster, StreamerConfigService), String> {
    let module = LiveModule::load(&app.ctx).unwrap();
    let (roster, service) = (module.roster(), module.config_service());
    let mut subsystem = module.into_subsystem(None).unwrap();
    let step = subsystem.boot_steps.remove(0);
    (step.run)(app.ctx.clone())
        .await
        .map_err(|e| e.to_string())?;
    Ok((roster, service))
}

#[tokio::test]
async fn imports_channels_json_once_and_fails_boot_when_it_is_invalid() {
    let mut app = omni_testkit::TestApp::new().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("channels.json");
    std::fs::File::create(&path)
        .unwrap()
        .write_all(br#"{"Destiny": {"kick": ""}}"#)
        .unwrap();
    let mut config = (*app.ctx.config).clone();
    config.channels_config_path = Some(path.display().to_string());
    app.ctx.config = std::sync::Arc::new(config);
    let error = boot(&app).await.err().expect("invalid config fails boot");
    assert!(error.contains("Invalid channels config"), "{error}");

    std::fs::write(
        &path,
        br#"{"Destiny": {"youtube": "@destiny", "pushoverToken": "abc123"}, "Jerma": {"twitch": "jerma985", "tier": "background"}, "dggTopEmbeds": 2}"#,
    )
    .unwrap();
    let (roster, service) = boot(&app).await.unwrap();
    let listed = service.list().await.unwrap();
    let names: Vec<&str> = listed
        .streamers
        .iter()
        .map(|s| s.display_name.as_str())
        .collect();
    assert_eq!(names, ["Destiny", "Jerma"]);
    assert!(listed.streamers[0].has_pushover_token);
    assert_eq!(listed.settings.dgg_top_embeds, 2);
    assert_eq!(roster.len(), 2);
    assert_eq!(
        roster.get("destiny").unwrap().pushover_token.as_deref(),
        Some("abc123")
    );

    // Later boots never read the file again.
    std::fs::write(&path, br#"{"Hutch": {"youtube": "@hutch"}}"#).unwrap();
    let (_, service) = boot(&app).await.unwrap();
    assert_eq!(service.list().await.unwrap().streamers.len(), 2);
}
