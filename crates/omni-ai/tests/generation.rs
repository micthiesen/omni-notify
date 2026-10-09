//! `Ai` helpers over scripted models: structured output, retries, the tool loop and the
//! cost events they record (compared with real production rows), plus the one-time
//! historical cost import.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::sync::Arc;

use omni_ai::costs::{CostEventData, CostRecorder, import_historical_costs};
use omni_ai::{
    Ai, AiError, ContentPart, CostTag, GenerateRequest, GenerateResponse, LanguageModel, ModelId,
    ModelOverride, ModelRole, Role, StepRecord, ToolCall, ToolSet, Usage,
};
use omni_api::costs::{CostCategory, CostPriceStatus};
use omni_core::clock::SharedClock;
use omni_store::cbor::{Extra, JsValue};
use omni_store::entity::{Entity, UpsertOpts};
use omni_store::{DocOps, EntityOps, EntityWrite, Store};
use omni_tasks::EventBus;
use omni_tasks::log_capture::{RunLogLayer, RunLogs, run_span};
use omni_testkit::{FakeFailure, FakeModels, FakeTool, TEST_EPOCH_MS, TestStore, test_clock};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::Instrument;
use tracing_subscriber::layer::SubscriberExt;

#[derive(Debug, Deserialize, schemars::JsonSchema, PartialEq)]
struct Verdict {
    relevant: bool,
}

struct Harness {
    _store: TestStore,
    store: Store,
    clock: SharedClock,
    ai: Ai,
    fakes: FakeModels,
}

async fn harness() -> Harness {
    let clock: SharedClock = test_clock(TEST_EPOCH_MS);
    let test_store = TestStore::new(clock.clone()).await;
    let store = test_store.store.clone();
    let fakes = FakeModels::default();
    let ai = Ai::with_keys(
        omni_testkit::no_network(),
        BTreeMap::new(),
        CostRecorder::new(store.clone(), clock.clone()),
    )
    .with_override(Arc::new(fakes.clone()));
    Harness {
        _store: test_store,
        store,
        clock,
        ai,
        fakes,
    }
}

impl Harness {
    fn model(&self, role: ModelRole) -> Arc<dyn LanguageModel> {
        let id = ModelId::parse(role.default_model()).unwrap();
        self.fakes.model(Some(role), &id).unwrap()
    }

    async fn costs(&self) -> Vec<CostEventData> {
        self.store
            .read(|docs| docs.get_all::<CostEventData>())
            .await
            .unwrap()
    }
}

fn usage(input: u64, cached: u64, output: u64, reasoning: u64) -> Usage {
    Usage {
        input_tokens: input,
        input_no_cache_tokens: input - cached,
        cache_read_tokens: cached,
        cache_write_tokens: 0,
        output_tokens: output,
        reasoning_tokens: reasoning,
    }
}

#[tokio::test(start_paused = true)]
async fn structured_output_records_one_priced_cost_event_per_call() {
    let h = harness().await;
    h.fakes.script(
        ModelRole::Triage,
        vec![GenerateResponse {
            usage: usage(1000, 200, 100, 40),
            ..GenerateResponse::text("{\"relevant\":true}")
        }],
    );
    let model = h.model(ModelRole::Triage);
    let (verdict, used) =
        h.ai.generate_object::<Verdict>(
            model.as_ref(),
            GenerateRequest::prompt("Is this relevant?"),
            CostTag::for_role(ModelRole::Triage),
        )
        .await
        .unwrap();
    assert_eq!(verdict, Verdict { relevant: true });
    assert_eq!(used.output_tokens, 100);

    let requests = h.fakes.requests();
    let schema = &requests[0].1.output.as_ref().unwrap().schema;
    assert_eq!(schema["additionalProperties"], false);

    let costs = h.costs().await;
    assert_eq!(costs.len(), 1);
    let event = &costs[0];
    assert_eq!(event.category, CostCategory::Llm);
    assert_eq!(event.feature, "email-triage");
    assert_eq!(event.operation, "classify");
    assert_eq!(event.service, "openai");
    assert_eq!(event.model.as_deref(), Some("gpt-6-luna"));
    assert_eq!(event.price_status, CostPriceStatus::Estimated);
    let expected = 1000.0 * 0.00001 + 100.0 * 0.00005;
    assert!((event.cost_cents.unwrap() - expected).abs() < 1e-12);
    assert_eq!(event.incurred_at, h.clock.now_ms());
    assert_eq!(event.run_id, None);
    assert_eq!(
        serde_json::to_value(&event.usage).unwrap(),
        json!({"inputTokens": 1000.0, "inputNoCacheTokens": 800.0, "cacheReadTokens": 200.0,
               "cacheWriteTokens": 0.0, "outputTokens": 100.0, "reasoningTokens": 40.0, "requests": 1.0})
    );

    // The stored row has TS key order and `runId: undefined`, like rows TS writes outside a run.
    let pk = omni_store::entity::pk::<CostEventData>(&event.event_id).unwrap();
    let raw = h
        .store
        .read(move |docs| docs.get_doc(&pk))
        .await
        .unwrap()
        .unwrap();
    let JsValue::Object(fields) = raw else {
        panic!("cost event is not an object");
    };
    let keys: Vec<&str> = fields.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        [
            "category",
            "feature",
            "operation",
            "service",
            "model",
            "costCents",
            "priceStatus",
            "usage",
            "eventId",
            "incurredAt",
            "runId"
        ]
    );
    assert_eq!(fields["runId"], JsValue::Undefined);
}

#[tokio::test]
async fn run_attribution_overrides_the_static_feature() {
    let h = harness().await;
    let logs = RunLogs::new(EventBus::new(16), h.clock.clone());
    let subscriber = tracing_subscriber::registry().with(RunLogLayer::new(logs));
    let _guard = tracing::subscriber::set_default(subscriber);
    h.fakes
        .script(ModelRole::RecsShortlist, vec![GenerateResponse::text("ok")]);
    let model = h.model(ModelRole::RecsShortlist);
    h.ai.generate_text(
        model.as_ref(),
        GenerateRequest::prompt("x"),
        CostTag::for_role(ModelRole::RecsShortlist),
    )
    .instrument(run_span("PodcastRecs:1:abc", "PodcastRecs"))
    .await
    .unwrap();
    let costs = h.costs().await;
    assert_eq!(costs[0].feature, "podcast-recommendations");
    assert_eq!(costs[0].run_id.as_deref(), Some("PodcastRecs:1:abc"));
    assert_eq!(costs[0].operation, "shortlist");
}

#[tokio::test(start_paused = true)]
async fn retryable_failures_are_retried_with_backoff() {
    let h = harness().await;
    h.fakes.script_failure(
        ModelRole::Briefing,
        FakeFailure {
            status: 503,
            message: "overloaded".to_owned(),
        },
    );
    h.fakes
        .script(ModelRole::Briefing, vec![GenerateResponse::text("done")]);
    let model = h.model(ModelRole::Briefing);
    let started = tokio::time::Instant::now();
    let (text, _) =
        h.ai.generate_text(
            model.as_ref(),
            GenerateRequest::prompt("x"),
            CostTag::for_role(ModelRole::Briefing),
        )
        .await
        .unwrap();
    assert_eq!(text, "done");
    assert_eq!(started.elapsed(), omni_ai::RETRY_INITIAL_DELAY);
    assert_eq!(
        h.costs().await.len(),
        1,
        "only the successful call is billed"
    );

    h.fakes.script_failure(
        ModelRole::Briefing,
        FakeFailure {
            status: 400,
            message: "bad request".to_owned(),
        },
    );
    let result =
        h.ai.generate_text(
            model.as_ref(),
            GenerateRequest::prompt("x"),
            CostTag::for_role(ModelRole::Briefing),
        )
        .await;
    assert!(matches!(result, Err(AiError::Provider { status: 400, .. })));
}

#[tokio::test]
async fn invalid_structured_output_is_a_schema_error() {
    let h = harness().await;
    h.fakes.script(
        ModelRole::Triage,
        vec![GenerateResponse::text("{\"relevant\":\"maybe\"}")],
    );
    let model = h.model(ModelRole::Triage);
    let result =
        h.ai.generate_object::<Verdict>(
            model.as_ref(),
            GenerateRequest::prompt("x"),
            CostTag::for_role(ModelRole::Triage),
        )
        .await;
    assert!(
        matches!(result, Err(AiError::Schema(message)) if message.starts_with("No object generated"))
    );
}

fn call(id: &str, name: &str, arguments: serde_json::Value) -> ToolCall {
    ToolCall {
        call_id: id.to_owned(),
        name: name.to_owned(),
        arguments,
    }
}

#[tokio::test]
async fn tool_loop_runs_tools_and_feeds_results_back() {
    let h = harness().await;
    let search = FakeTool::new(
        "web_search",
        vec![Ok(json!({"results": [], "responseTime": 0.1}))],
    );
    let tools = ToolSet::new().with(Arc::new(search.clone()));
    h.fakes.script(
        ModelRole::Workspace,
        vec![
            GenerateResponse {
                usage: usage(100, 0, 10, 0),
                ..GenerateResponse::tool_calls(vec![
                    call("c1", "web_search", json!({"query": "rust"})),
                    call("c2", "report_papercut", json!({})),
                ])
            },
            GenerateResponse {
                usage: usage(200, 0, 20, 0),
                ..GenerateResponse::text("{\"relevant\":false}")
            },
        ],
    );
    let model = h.model(ModelRole::Workspace);
    let mut steps: Vec<StepRecord> = Vec::new();
    let result =
        h.ai.run_tool_loop(
            model.as_ref(),
            GenerateRequest::prompt("research"),
            &tools,
            12,
            CostTag::with_operation(ModelRole::Workspace, "run"),
            &mut |step| steps.push(step.clone()),
        )
        .await
        .unwrap();
    assert_eq!(result.steps, 2);
    assert!(!result.stopped_at_step_limit);
    assert_eq!(result.usage.input_tokens, 300);
    assert_eq!(
        result.object::<Verdict>().unwrap(),
        Verdict { relevant: false }
    );
    assert_eq!(search.calls(), vec![json!({"query": "rust"})]);
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0].tool_results[0].0, "web_search");
    assert!(steps[0].tool_results[1].1.is_err());

    let requests = h.fakes.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].1.tools[0].name, "web_search");
    let second = &requests[1].1.messages;
    assert_eq!(second.len(), 3);
    assert_eq!(second[1].role, Role::Assistant);
    assert_eq!(second[2].role, Role::Tool);
    assert_eq!(
        second[2].content[1],
        ContentPart::ToolResult {
            call_id: "c2".to_owned(),
            name: "report_papercut".to_owned(),
            output: json!("Model tried to call unavailable tool 'report_papercut'"),
            is_error: true,
        }
    );
    let costs = h.costs().await;
    assert_eq!(costs.len(), 2);
    assert!(
        costs
            .iter()
            .all(|c| c.feature == "workspaces" && c.operation == "run")
    );
}

#[tokio::test]
async fn tool_loop_stops_at_the_step_limit() {
    let h = harness().await;
    let tool = FakeTool::new("inspect_target", vec![Ok(json!({})), Ok(json!({}))]);
    let tools = ToolSet::new().with(Arc::new(tool.clone()));
    h.fakes.script(
        ModelRole::ObserverRepair,
        vec![
            GenerateResponse::tool_calls(vec![call("a", "inspect_target", json!({}))]),
            GenerateResponse::tool_calls(vec![call("b", "inspect_target", json!({}))]),
        ],
    );
    let model = h.model(ModelRole::ObserverRepair);
    let result =
        h.ai.run_tool_loop(
            model.as_ref(),
            GenerateRequest::prompt("repair"),
            &tools,
            2,
            CostTag::for_role(ModelRole::ObserverRepair),
            &mut |_| {},
        )
        .await
        .unwrap();
    assert!(result.stopped_at_step_limit);
    assert_eq!(result.steps, 2);
    assert_eq!(tool.calls().len(), 2, "the last step's tools still run");
    assert!(matches!(
        result.object::<Verdict>(),
        Err(AiError::StepLimit)
    ));
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BriefingHistory {
    briefing_name: String,
    notifications: Vec<serde_json::Value>,
    #[serde(flatten)]
    extra: Extra,
}

impl Entity for BriefingHistory {
    const NAME: &'static str = "briefing-history";
    type Key = String;
    fn key(&self) -> String {
        self.briefing_name.clone()
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Episode {
    episode_id: String,
    created_at: i64,
    run_id: String,
    voice_provider: String,
    costs: serde_json::Value,
}

impl Entity for Episode {
    const NAME: &'static str = "press-pods-episode";
    type Key = String;
    fn key(&self) -> String {
        self.episode_id.clone()
    }
}

#[tokio::test]
async fn historical_costs_are_imported_once() {
    let h = harness().await;
    h.store
        .write(|tx| {
            tx.upsert(
                &BriefingHistory {
                    briefing_name: "Morning".to_owned(),
                    notifications: vec![
                        json!({"title": "a", "timestamp": 1000, "costCents": 1.5, "runId": "Morning:1"}),
                        json!({"title": "b", "timestamp": 2000}),
                        json!({"title": "c", "timestamp": 3000, "costCents": null}),
                    ],
                    extra: Extra::new(),
                },
                UpsertOpts::default(),
            )?;
            tx.upsert(
                &Episode {
                    episode_id: "ep1".to_owned(),
                    created_at: 5000,
                    run_id: "PressPods:5".to_owned(),
                    voice_provider: "Higgs".to_owned(),
                    costs: json!({
                        "llmCents": 3.5, "ttsCents": 0, "detailCents": {},
                        "detailTokens": {"gpt-6-luna-meta": {"input": 10, "output": 5}, "gpt-6-sol-clean": {"input": 1, "output": 2}},
                        "detailChars": {"bosonai/higgs-audio-v3-tts-4b-tts": 2585}
                    }),
                },
                UpsertOpts::default(),
            )
        })
        .await
        .unwrap();
    assert_eq!(import_historical_costs(&h.store).await.unwrap(), 4);
    assert_eq!(import_historical_costs(&h.store).await.unwrap(), 0);
    let mut costs = h.costs().await;
    costs.sort_by(|a, b| a.event_id.cmp(&b.event_id));
    let ids: Vec<&str> = costs.iter().map(|c| c.event_id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "legacy:briefing:Morning:1000:0",
            "legacy:briefing:Morning:3000:2",
            "legacy:press-pods:llm:ep1",
            "legacy:press-pods:tts:ep1"
        ]
    );
    assert_eq!(costs[0].cost_cents, Some(1.5));
    assert_eq!(costs[0].run_id.as_deref(), Some("Morning:1"));
    assert_eq!(costs[1].price_status, CostPriceStatus::Unknown);
    assert_eq!(costs[2].cost_cents, Some(3.5));
    assert_eq!(costs[2].usage.input_tokens, Some(11.0));
    assert_eq!(costs[3].service, "higgs");
    assert_eq!(costs[3].price_status, CostPriceStatus::Free);
    assert_eq!(costs[3].usage.characters, Some(2585.0));
}
