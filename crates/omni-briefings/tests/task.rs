//! Briefing agent task behavior: the
//! `send_notification` tool reserves before pushing, suppresses duplicates,
//! releases on a confirmed failure, and the run backfills its cost.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use omni_ai::{GenerateResponse, ModelRole, ToolCall, Usage};
use omni_alerts::PushoverMessage;
use omni_briefings::configs::parse_briefing;
use omni_briefings::persistence::{CostCents, get_history};
use omni_briefings::{BriefingDeps, BriefingNotifier, BriefingTask};
use omni_tasks::{RunContext, Task, Trigger};
use omni_testkit::{FakeTool, TestApp, test_app_env};
use serde_json::json;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct Recorder {
    sent: Mutex<Vec<PushoverMessage>>,
    fail: Mutex<bool>,
}

impl BriefingNotifier for Recorder {
    fn send<'a>(&'a self, message: PushoverMessage) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            if *self.fail.lock().unwrap() {
                return Err("Pushover unavailable".to_owned());
            }
            self.sent.lock().unwrap().push(message);
            Ok(())
        })
    }
}

fn send_call(id: &str, title: &str) -> ToolCall {
    ToolCall {
        call_id: id.to_owned(),
        name: "send_notification".to_owned(),
        arguments: json!({
            "title": title,
            "message": "Body",
            "url": "https://example.com/story",
            "url_title": "Read more",
        }),
    }
}

fn cx(run_id: &str) -> RunContext {
    RunContext {
        run_id: run_id.to_owned(),
        task_name: "News".to_owned(),
        trigger: Trigger::Manual,
        scheduled_for: None,
        cancel: CancellationToken::new(),
    }
}

async fn setup(recorder: Arc<Recorder>) -> (TestApp, BriefingTask) {
    let app = TestApp::new().await;
    let mut env = test_app_env();
    env.insert("TAVILY_API_KEY".to_owned(), "test-key".to_owned());
    let config = Arc::new(omni_config::Config::from_env(&env).unwrap());
    let tz = jiff::tz::TimeZone::get("America/Vancouver").unwrap();
    let deps = BriefingDeps {
        store: app.ctx.store.clone(),
        clock: app.ctx.clock.clone(),
        ai: app.ctx.ai.clone(),
        config,
        tz: tz.clone(),
        notifier: recorder,
        web_search: Arc::new(FakeTool::new("web_search", vec![])),
        fetch_url: Arc::new(FakeTool::new("fetch_url", vec![])),
        logs_path: None,
    };
    let briefing = parse_briefing(
        "News",
        "---\nschedule: \"0 0 8 * * *\"\n---\nBrief me.",
        &tz,
    )
    .unwrap();
    let task = BriefingTask::create(briefing, deps).expect("enabled with a Tavily key");
    (app, task)
}

#[tokio::test]
async fn sends_once_records_history_and_backfills_cost() {
    let recorder = Arc::new(Recorder::default());
    let (app, task) = setup(recorder.clone()).await;
    let usage = Usage {
        input_tokens: 1_000,
        output_tokens: 200,
        ..Usage::default()
    };
    app.ai.script(
        ModelRole::Briefing,
        vec![
            GenerateResponse {
                usage,
                ..GenerateResponse::tool_calls(vec![
                    send_call("1", "Story"),
                    send_call("2", "Story"),
                ])
            },
            GenerateResponse {
                usage,
                ..GenerateResponse::text("Done")
            },
        ],
    );
    task.run(&cx("News:run-1")).await.unwrap();

    // The duplicate call in the same run is suppressed by its reservation.
    assert_eq!(recorder.sent.lock().unwrap().len(), 1);
    let history = get_history(&app.ctx.store, "News").await.unwrap();
    assert_eq!(history.notifications.len(), 1);
    let stored = &history.notifications[0];
    assert_eq!(stored.run_id.as_deref(), Some("News:run-1"));
    let total = Usage {
        input_tokens: 2_000,
        output_tokens: 400,
        ..Usage::default()
    };
    let expected = omni_ai::costs::llm_cost_cents("gpt-6-luna", &total);
    assert_eq!(
        stored.cost_cents,
        expected.map_or(CostCents::Unpriced, CostCents::Cents)
    );
    let (_, request) = &app.ai.requests()[0];
    assert_eq!(
        request.reasoning_effort,
        Some(omni_ai::ReasoningEffort::High)
    );
    let names: Vec<String> = request.tools.iter().map(|t| t.name.clone()).collect();
    assert!(names.contains(&"send_notification".to_owned()));
    assert!(names.contains(&"web_search".to_owned()));
}

#[tokio::test]
async fn releases_the_reservation_when_pushover_fails() {
    let recorder = Arc::new(Recorder::default());
    *recorder.fail.lock().unwrap() = true;
    let (app, task) = setup(recorder.clone()).await;
    app.ai.script(
        ModelRole::Briefing,
        vec![
            GenerateResponse::tool_calls(vec![send_call("1", "Story")]),
            GenerateResponse::text("Gave up"),
        ],
    );
    task.run(&cx("News:run-2")).await.unwrap();
    let history = get_history(&app.ctx.store, "News").await.unwrap();
    assert!(history.notifications.is_empty());
    let id = omni_briefings::task::delivery_id(
        "News:run-2",
        "Story",
        "Body",
        "https://example.com/story",
    );
    assert!(
        omni_briefings::persistence::reserve_delivery(&app.ctx.store, "News", &id)
            .await
            .unwrap(),
        "a confirmed failure releases the reservation for retry"
    );
}

#[tokio::test]
async fn disabled_without_tavily_key() {
    let app = TestApp::new().await;
    let tz = jiff::tz::TimeZone::get("America/Vancouver").unwrap();
    let deps = omni_briefings::deps(&app.ctx).unwrap();
    let briefing = parse_briefing(
        "News",
        "---\nschedule: \"0 0 8 * * *\"\n---\nBrief me.",
        &tz,
    )
    .unwrap();
    assert!(BriefingTask::create(briefing, deps).is_none());
}

#[test]
fn delivery_ids_hash_the_js_json_of_the_content() {
    let id = omni_briefings::task::delivery_id("run", "Title", "Msg", "https://x.y");
    let hash =
        omni_core::digest::sha256_hex(br#"{"title":"Title","message":"Msg","url":"https://x.y"}"#);
    assert_eq!(id, format!("run:{}", &hash[..24]));
}
