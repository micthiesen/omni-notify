//! Tracked streamers are configured in the database through the REST routes
//! (UI) and MCP tools; changes reach the live roster immediately, and Pushover
//! tokens are write-only and never accepted over MCP.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use axum::Router;
use axum::http::Method;
use omni_live::{LiveModule, Roster, StreamerConfigService};
use omni_mcp_kit::registry::standalone_context;
use omni_mcp_kit::{McpTool, ToolOutput, ToolPhase};
use omni_testkit::TestApp;
use serde_json::{Value, json};

struct Fixture {
    app: TestApp,
    router: Router,
    roster: Roster,
    service: StreamerConfigService,
    tools: Vec<McpTool>,
    _dir: tempfile::TempDir,
}

/// A booted module with no `channels.json` (an empty configuration).
async fn fixture() -> Fixture {
    let mut app = TestApp::new().await;
    let dir = tempfile::tempdir().unwrap();
    let mut config = (*app.ctx.config).clone();
    config.channels_config_path = Some(dir.path().join("channels.json").display().to_string());
    app.ctx.config = std::sync::Arc::new(config);
    let module = LiveModule::load(&app.ctx).unwrap();
    let (roster, service) = (module.roster(), module.config_service());
    let mut subsystem = module.into_subsystem(None).unwrap();
    let step = subsystem.boot_steps.remove(0);
    (step.run)(app.ctx.clone()).await.unwrap();
    let router = app.router(&subsystem);
    let tools = std::mem::take(&mut subsystem.mcp_tools);
    Fixture {
        app,
        router,
        roster,
        service,
        tools,
        _dir: dir,
    }
}

impl Fixture {
    async fn send(&self, method: Method, path: &str, body: Option<Value>) -> (u16, Value) {
        let (status, body) = self
            .app
            .request_json(&self.router, method, path, body.as_ref())
            .await;
        (status.as_u16(), body)
    }

    async fn call(&self, name: &str, input: Value) -> Result<Value, (ToolPhase, String)> {
        let tool = self.tools.iter().find(|t| t.meta.name == name).unwrap();
        match tool.handler.call(input, standalone_context("test")).await {
            Ok(ToolOutput::Structured(map)) => Ok(Value::Object(map)),
            Ok(ToolOutput::Custom { structured, .. }) => Ok(Value::Object(structured)),
            Err(e) => Err((e.phase, e.message)),
        }
    }

    fn roster_names(&self) -> Vec<String> {
        self.roster
            .snapshot()
            .into_iter()
            .map(|s| s.display_name)
            .collect()
    }
}

#[tokio::test]
async fn the_ui_creates_edits_reorders_and_deletes_streamers() {
    let f = fixture().await;
    let (status, created) = f
        .send(
            Method::POST,
            "/api/streamer-config/streamers",
            Some(json!({
                "displayName": " Destiny ",
                "youtube": ["@destiny", "@DESTINY", "@destinyclips"],
                "twitch": ["destiny"],
                "pushoverToken": "abc123"
            })),
        )
        .await;
    assert_eq!(status, 200, "{created}");
    assert_eq!(
        created,
        json!({
            "id": "destiny", "displayName": "Destiny",
            "youtube": ["@destiny", "@destinyclips"], "twitch": ["destiny"], "kick": [],
            "tier": "primary", "liveNotifications": null, "hasPushoverToken": true
        })
    );
    let (status, _) = f
        .send(
            Method::POST,
            "/api/streamer-config/streamers",
            Some(json!({"displayName": "Jerma", "twitch": ["jerma985"], "tier": "background"})),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(f.roster_names(), ["Destiny", "Jerma"]);
    let destiny = f.roster.snapshot().into_iter().next().unwrap();
    assert_eq!(destiny.bindings.len(), 3);
    assert_eq!(destiny.pushover_token.as_deref(), Some("abc123"));

    // A binding already owned by another streamer is rejected.
    let (status, body) = f
        .send(
            Method::POST,
            "/api/streamer-config/streamers",
            Some(json!({"displayName": "Copy", "twitch": ["JERMA985"]})),
        )
        .await;
    assert_eq!(status, 400, "{body}");

    // Background with an explicit live-notification override is contradictory.
    let (status, _) = f
        .send(
            Method::PATCH,
            "/api/streamer-config/streamers/jerma",
            Some(json!({"liveNotifications": true})),
        )
        .await;
    assert_eq!(status, 400);

    // Promoting to primary, clearing the token and renaming keep the id.
    let (status, edited) = f
        .send(
            Method::PATCH,
            "/api/streamer-config/streamers/destiny",
            Some(json!({"displayName": "Steven", "pushoverToken": "", "liveNotifications": false})),
        )
        .await;
    assert_eq!(status, 200, "{edited}");
    assert_eq!(edited["id"], "destiny");
    assert_eq!(edited["hasPushoverToken"], false);
    assert_eq!(edited["liveNotifications"], false);
    assert_eq!(f.roster.snapshot()[0].pushover_token, None);

    let (status, listed) = f
        .send(
            Method::PUT,
            "/api/streamer-config/order",
            Some(json!({"ids": ["jerma", "destiny"]})),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(listed["streamers"][0]["id"], "jerma");
    assert_eq!(f.roster_names(), ["Jerma", "Steven"]);
    let (status, _) = f
        .send(
            Method::PUT,
            "/api/streamer-config/order",
            Some(json!({"ids": ["jerma"]})),
        )
        .await;
    assert_eq!(status, 400);

    let (status, settings) = f
        .send(
            Method::PUT,
            "/api/streamer-config/settings",
            Some(json!({"dggTopEmbeds": 3})),
        )
        .await;
    assert_eq!((status, settings), (200, json!({"dggTopEmbeds": 3})));

    let (status, _) = f
        .send(Method::DELETE, "/api/streamer-config/streamers/jerma", None)
        .await;
    assert_eq!(status, 200);
    let (status, _) = f
        .send(Method::DELETE, "/api/streamer-config/streamers/jerma", None)
        .await;
    assert_eq!(status, 404);
    assert_eq!(f.roster_names(), ["Steven"]);

    let (status, listed) = f.app.get_json(&f.router, "/api/streamer-config").await;
    assert_eq!(status, 200);
    assert_eq!(listed["settings"], json!({"dggTopEmbeds": 3}));
    assert_eq!(listed["streamers"].as_array().unwrap().len(), 1);
    assert!(!listed.to_string().contains("abc123"));
}

#[tokio::test]
async fn mcp_tools_manage_streamers_but_never_accept_pushover_tokens() {
    let f = fixture().await;
    f.service
        .create(omni_api::streamer_config::StreamerConfigCreate {
            display_name: "Vinesauce".into(),
            twitch: vec!["vinesauce".into()],
            pushover_token: Some("secret1".into()),
            ..Default::default()
        })
        .await
        .unwrap();

    let error = f
        .call(
            "streamer_config_create",
            json!({"displayName": "Hutch", "youtube": ["@hutch"], "pushoverToken": "x1"}),
        )
        .await
        .unwrap_err();
    assert!(matches!(error.0, ToolPhase::Input), "{error:?}");
    let error = f
        .call(
            "streamer_config_update",
            json!({"streamerId": "vinesauce", "pushoverToken": ""}),
        )
        .await
        .unwrap_err();
    assert!(matches!(error.0, ToolPhase::Input), "{error:?}");

    let created = f
        .call(
            "streamer_config_create",
            json!({"displayName": "Hutch", "youtube": ["@hutch"], "kick": ["hutch"]}),
        )
        .await
        .unwrap();
    assert_eq!(created["streamer"]["id"], "hutch");

    // An MCP edit keeps the token set in the UI.
    let updated = f
        .call(
            "streamer_config_update",
            json!({"streamerId": "vinesauce", "tier": "background", "twitch": ["vinesauce", "vinny"]}),
        )
        .await
        .unwrap();
    assert_eq!(updated["streamer"]["tier"], "background");
    assert_eq!(updated["streamer"]["hasPushoverToken"], true);
    assert_eq!(updated["streamer"]["twitch"], json!(["vinesauce", "vinny"]));

    let error = f
        .call(
            "streamer_config_update",
            json!({"streamerId": "nobody", "tier": "primary"}),
        )
        .await
        .unwrap_err();
    assert!(error.1.contains("nobody"), "{error:?}");

    let listed = f
        .call(
            "streamer_configs_reorder",
            json!({"streamerIds": ["hutch", "vinesauce"]}),
        )
        .await
        .unwrap();
    assert_eq!(listed["streamers"][0]["id"], "hutch");
    // TestApp has no Kick credentials: the Kick source is stored but not polled.
    assert_eq!(listed["kickConfigured"], false);
    assert_eq!(f.roster.snapshot()[0].bindings.len(), 1);

    let settings = f
        .call("livestream_settings_update", json!({"dggTopEmbeds": 1}))
        .await
        .unwrap();
    assert_eq!(settings, json!({"settings": {"dggTopEmbeds": 1}}));

    let deleted = f
        .call("streamer_config_delete", json!({"streamerId": "hutch"}))
        .await
        .unwrap();
    assert_eq!(deleted["streamer"]["displayName"], "Hutch");
    let listed = f.call("streamer_configs_list", json!({})).await.unwrap();
    assert_eq!(listed["streamers"].as_array().unwrap().len(), 1);
    assert!(!listed.to_string().contains("secret1"));
    assert_eq!(f.roster_names(), ["Vinesauce"]);
}

#[tokio::test]
async fn configuration_survives_a_restart_without_the_file() {
    let f = fixture().await;
    f.service
        .create(omni_api::streamer_config::StreamerConfigCreate {
            display_name: "Jerma".into(),
            twitch: vec!["jerma985".into()],
            ..Default::default()
        })
        .await
        .unwrap();
    let module = LiveModule::load(&f.app.ctx).unwrap();
    let roster = module.roster();
    let mut subsystem = module.into_subsystem(None).unwrap();
    let step = subsystem.boot_steps.remove(0);
    (step.run)(f.app.ctx.clone()).await.unwrap();
    let names: Vec<String> = roster
        .snapshot()
        .into_iter()
        .map(|s| s.display_name)
        .collect();
    assert_eq!(names, ["Jerma"]);
}

#[tokio::test]
async fn a_missing_file_does_not_block_a_later_import() {
    let f = fixture().await;
    std::fs::write(
        f._dir.path().join("channels.json"),
        br#"{"Jerma": {"twitch": "jerma985"}, "dggTopEmbeds": 2}"#,
    )
    .unwrap();
    let module = LiveModule::load(&f.app.ctx).unwrap();
    let service = module.config_service();
    let mut subsystem = module.into_subsystem(None).unwrap();
    let step = subsystem.boot_steps.remove(0);
    (step.run)(f.app.ctx.clone()).await.unwrap();
    let listed = service.list().await.unwrap();
    assert_eq!(listed.streamers.len(), 1);
    assert_eq!(listed.settings.dgg_top_embeds, 2);
}

#[tokio::test]
async fn usernames_are_checked_per_platform() {
    let f = fixture().await;
    for (body, ok) in [
        (json!({"displayName": "A", "twitch": ["bad/name"]}), false),
        (json!({"displayName": "A", "kick": ["a?b"]}), false),
        (json!({"displayName": "A", "youtube": ["../x"]}), false),
        (json!({"displayName": "A", "youtube": ["@Whick-TV"]}), true),
        // Channel ids are case-sensitive, so these are two different sources.
        (
            json!({"displayName": "B", "youtube": ["channel/UCabc"]}),
            true,
        ),
        (
            json!({"displayName": "C", "youtube": ["channel/UCABC"]}),
            true,
        ),
        (json!({"displayName": "D", "youtube": ["@whick-tv"]}), false),
    ] {
        let (status, response) = f
            .send(
                Method::POST,
                "/api/streamer-config/streamers",
                Some(body.clone()),
            )
            .await;
        assert_eq!(status == 200, ok, "{body} -> {response}");
    }
}
