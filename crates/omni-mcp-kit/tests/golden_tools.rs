//! Golden tool metadata, schema validation and TS-compatible result formatting.
#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use futures::future::BoxFuture;
use omni_mcp_kit::golden::golden_tools;
use omni_mcp_kit::registry::{
    RegistryError, RegistryServer, ToolRegistry, call_tool_result, standalone_context,
};
use omni_mcp_kit::{
    ExecutorPolicy, McpTool, SchemaValidator, ToolContext, ToolError, ToolHandler, ToolMetaError,
    ToolOutput, ToolPhase, golden_meta, raw_tool, typed_tool,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

#[test]
fn golden_lists_every_policy_tool_with_metadata() {
    let golden = golden_tools().unwrap();
    assert_eq!(golden.metas.len(), 101);
    let policy: Value =
        serde_json::from_str(include_str!("../../../docs/mcp-policy.json")).unwrap();
    assert_eq!(
        policy,
        serde_json::from_str::<Value>(omni_mcp_kit::golden::POLICY_JSON).unwrap(),
        "golden/mcp-policy.json must equal docs/mcp-policy.json"
    );
    for entry in policy["tools"].as_array().unwrap() {
        let meta = golden_meta(entry["name"].as_str().unwrap()).unwrap();
        assert_eq!(meta.title, entry["title"].as_str().unwrap());
        assert_eq!(meta.description, entry["description"].as_str().unwrap());
        assert_eq!(
            serde_json::to_value(meta.annotations).unwrap(),
            entry["annotations"]
        );
        assert_eq!(meta.policy.cost, entry["cost"].as_str().unwrap());
    }
    let first = &golden.metas[0];
    assert_eq!(first.name, "list_reminder_lists");
    let compose = golden_meta("email_send").unwrap();
    assert_eq!(
        compose.policy.recommended_policy,
        ExecutorPolicy::RequireApproval
    );
}

#[test]
fn every_golden_schema_compiles() {
    for meta in &golden_tools().unwrap().metas {
        SchemaValidator::new(&meta.name, &meta.input_schema).unwrap();
        SchemaValidator::new(&meta.name, &meta.output_schema).unwrap();
    }
}

#[test]
fn unknown_tools_have_no_metadata() {
    assert!(matches!(
        golden_meta("not_a_tool"),
        Err(ToolMetaError::UnknownTool(name)) if name == "not_a_tool"
    ));
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BriefingsInput {
    briefing_name: Option<String>,
    #[serde(default)]
    cursor: u32,
    #[serde(default = "default_limit")]
    limit: u32,
}

fn default_limit() -> u32 {
    25
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BriefingsOutput {
    briefing_names: Vec<String>,
    notifications: Vec<Value>,
    next_cursor: Option<u32>,
    total: u32,
}

fn briefings_tool() -> McpTool {
    typed_tool("briefings_list", |input: BriefingsInput, _cx| async move {
        Ok::<_, ToolError>(BriefingsOutput {
            briefing_names: input.briefing_name.into_iter().collect(),
            notifications: Vec::new(),
            next_cursor: None,
            total: input.cursor + input.limit,
        })
    })
    .unwrap()
}

fn cx() -> ToolContext {
    standalone_context("test")
}

#[tokio::test]
async fn typed_tools_validate_input_against_the_golden_schema() {
    let tool = briefings_tool();
    let ok = tool
        .handler
        .call(json!({"briefingName": "Daily"}), cx())
        .await;
    match ok {
        Ok(ToolOutput::Structured(map)) => {
            assert_eq!(map["total"], json!(25));
            assert_eq!(map["briefingNames"], json!(["Daily"]));
        }
        other => panic!("unexpected {other:?}"),
    }
    let too_big = tool.handler.call(json!({"limit": 101}), cx()).await;
    assert!(matches!(
        too_big,
        Err(ToolError {
            phase: ToolPhase::Input,
            ..
        })
    ));
    let extra = tool.handler.call(json!({"unknown": true}), cx()).await;
    assert!(matches!(
        extra,
        Err(ToolError {
            phase: ToolPhase::Input,
            ..
        })
    ));
}

#[tokio::test]
async fn typed_tools_validate_output_against_the_golden_schema() {
    let tool = typed_tool("briefings_list", |_: BriefingsInput, _cx| async move {
        Ok::<_, ToolError>(json!({"briefingNames": "not-an-array"}))
    })
    .unwrap();
    let result = tool.handler.call(json!({}), cx()).await;
    assert!(matches!(
        result,
        Err(ToolError {
            phase: ToolPhase::Output,
            ..
        })
    ));
}

struct Failing;

impl ToolHandler for Failing {
    fn call<'a>(
        &'a self,
        _input: Value,
        _cx: ToolContext,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async { Err(ToolError::execute("innermost cause")) })
    }
}

#[tokio::test]
async fn registry_orders_by_golden_list_and_rejects_duplicates() {
    let failing = raw_tool("browse_browser_history", Arc::new(Failing)).unwrap();
    let registry = ToolRegistry::new(vec![failing.clone(), briefings_tool()]).unwrap();
    let names: Vec<&str> = registry.listed().iter().map(|t| t.name.as_ref()).collect();
    assert_eq!(names, ["briefings_list", "browse_browser_history"]);
    assert!(matches!(
        registry.verify_complete(),
        Err(RegistryError::GoldenMismatch { missing, unexpected }) if missing.len() == 99 && unexpected.is_empty()
    ));
    assert!(matches!(
        ToolRegistry::new(vec![failing.clone(), failing]),
        Err(RegistryError::Duplicate(name)) if name == "browse_browser_history"
    ));
    let failed = registry
        .call("browse_browser_history", None, cx())
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(call_tool_result(failed)).unwrap(),
        json!({"content": [{"type": "text", "text": "innermost cause"}], "isError": true})
    );
    assert!(registry.call("nope", None, cx()).await.is_none());
}

#[test]
fn listed_tools_equal_the_golden_json() {
    let golden = golden_tools().unwrap();
    let registry = ToolRegistry::new(vec![briefings_tool()]).unwrap();
    let listed = serde_json::to_value(&registry.listed()[0]).unwrap();
    let raw = Value::Object(golden.get("briefings_list").unwrap().1.clone());
    assert_eq!(listed, raw);
}

#[test]
fn success_results_mirror_successful_tool_result() {
    let mut structured = Map::new();
    structured.insert("b".to_owned(), json!(1));
    structured.insert("a".to_owned(), json!("x"));
    let result = call_tool_result(Ok(ToolOutput::Structured(structured)));
    assert_eq!(
        serde_json::to_value(result).unwrap(),
        json!({
            "content": [{"type": "text", "text": "{\"b\":1,\"a\":\"x\"}"}],
            "structuredContent": {"b": 1, "a": "x"}
        })
    );
}

#[test]
fn registry_server_uses_golden_server_info() {
    let server = RegistryServer::new(ToolRegistry::new(Vec::new()).unwrap()).unwrap();
    let info = rmcp::handler::server::ServerHandler::get_info(&server);
    assert_eq!(info.server_info.name, "omni");
    assert!(
        info.instructions
            .unwrap()
            .starts_with("Omni exposes private personal data")
    );
}
