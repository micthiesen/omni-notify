//! Workspace MCP tools. Every call goes through
//! `typed_tool`, so inputs and outputs are validated against the golden schemas.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::Harness;
use omni_api::workspaces::{
    WorkspaceActionType, WorkspaceMessageRole, WorkspacePapercutCategory, WorkspaceSubjectStatus,
};
use omni_mcp_kit::{ToolContext, ToolError, ToolOutput, ToolPhase};
use omni_workspaces::persistence::{NewAction, NewArtifactRevision, NewMessage, NewPapercut};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

async fn call(h: &Harness, name: &str, input: Value) -> Result<Value, ToolError> {
    let tools = omni_workspaces::mcp::tools(&h.service).unwrap();
    let tool = tools.iter().find(|t| t.meta.name == name).unwrap();
    let cx = ToolContext {
        call_id: "1".to_owned(),
        cancel: CancellationToken::new(),
    };
    match tool.handler.call(input, cx).await? {
        ToolOutput::Structured(map) => Ok(Value::Object(map)),
        ToolOutput::Custom { structured, .. } => Ok(Value::Object(structured)),
    }
}

async fn seed(h: &Harness) -> (String, String) {
    h.subject(
        "purchase-research",
        "camera",
        WorkspaceSubjectStatus::Active,
    )
    .await;
    h.repo()
        .add_artifact_revision(NewArtifactRevision {
            workspace_id: "purchase-research".to_owned(),
            subject_id: "camera".to_owned(),
            artifact_key: "brief".to_owned(),
            kind: omni_api::workspaces::WorkspaceArtifactKind::Markdown,
            content: format!("Budget {} mirrorless", "x".repeat(300)),
            summary: "Brief".to_owned(),
            run_id: None,
        })
        .await
        .unwrap();
    h.repo()
        .add_message(NewMessage {
            workspace_id: "purchase-research".to_owned(),
            subject_id: Some("camera".to_owned()),
            role: WorkspaceMessageRole::User,
            text: "Find a mirrorless camera".to_owned(),
            run_id: None,
        })
        .await
        .unwrap();
    let (action, _) = h
        .repo()
        .add_action(NewAction {
            workspace_id: "purchase-research".to_owned(),
            subject_id: "camera".to_owned(),
            action_type: WorkspaceActionType::CalendarEvent,
            title: "Return reminder".to_owned(),
            description: "Return window".to_owned(),
            payload:
                json!({"title": "Return", "startDate": "2026-09-01", "allDay": true, "extra": 1})
                    .to_string(),
            run_id: None,
        })
        .await
        .unwrap();
    let papercut = h
        .repo()
        .report_papercut(NewPapercut {
            workspace_id: "purchase-research".to_owned(),
            subject_id: Some("camera".to_owned()),
            run_id: Some("r".to_owned()),
            category: WorkspacePapercutCategory::PoorSourceData,
            title: "Stale prices".to_owned(),
            detail: "d".to_owned(),
            related_tool: None,
        })
        .await
        .unwrap();
    (action.action_id, papercut.papercut_id)
}

#[tokio::test]
async fn registers_the_ten_tools_in_serving_order() {
    let h = Harness::new().await;
    let names: Vec<String> = omni_workspaces::mcp::tools(&h.service)
        .unwrap()
        .iter()
        .map(|t| t.meta.name.clone())
        .collect();
    assert_eq!(
        names,
        [
            "workspaces_list",
            "workspace_get",
            "workspace_search",
            "workspace_message",
            "workspace_subject_set_status",
            "workspace_actions_list",
            "workspace_action_approve",
            "workspace_action_reject",
            "workspace_papercuts_list",
            "workspace_papercut_resolve",
        ]
    );
}

#[tokio::test]
async fn lists_and_gets_bounded_dossiers() {
    let h = Harness::new().await;
    seed(&h).await;
    let listed = call(&h, "workspaces_list", json!({})).await.unwrap();
    assert_eq!(listed["workspaces"][0]["scheduledRuns"], true);
    assert_eq!(listed["workspaces"][1]["scheduledRuns"], false);
    assert_eq!(listed["workspaces"][0]["pendingActionCount"], 1);
    assert!(listed["workspaces"][0].get("instructions").is_none());

    let got = call(
        &h,
        "workspace_get",
        json!({"workspaceId": " purchase-research ", "subjectId": "camera", "maxContentChars": 200}),
    )
    .await
    .unwrap();
    assert_eq!(got["subject"]["subjectId"], "camera");
    assert_eq!(got["artifacts"][0]["contentTruncated"], true);
    assert_eq!(
        got["artifacts"][0]["content"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        200
    );
    assert_eq!(got["artifacts"][0]["runId"], Value::Null);
    assert_eq!(got["messages"][0]["subjectId"], "camera");
    assert_eq!(
        got["actions"][0]["payload"],
        json!({"title": "Return", "startDate": "2026-09-01", "allDay": true})
    );
    assert_eq!(got["emailScope"], Value::Null);
    assert!(got["papercuts"][0].get("fingerprint").is_none());

    let overview = call(
        &h,
        "workspace_get",
        json!({"workspaceId": "purchase-research"}),
    )
    .await
    .unwrap();
    assert_eq!(overview["subject"], Value::Null);
    assert_eq!(overview["messages"], json!([]));

    let missing = call(&h, "workspace_get", json!({"workspaceId": "nope"}))
        .await
        .unwrap_err();
    assert_eq!(missing.message, "Unknown workspace \"nope\"");
    let unknown_subject = call(
        &h,
        "workspace_get",
        json!({"workspaceId": "purchase-research", "subjectId": "x"}),
    )
    .await
    .unwrap_err();
    assert_eq!(
        unknown_subject.message,
        "Unknown subject \"x\" in workspace \"purchase-research\""
    );
    let invalid = call(&h, "workspace_get", json!({"workspaceId": "a", "bogus": 1}))
        .await
        .unwrap_err();
    assert_eq!(invalid.phase, ToolPhase::Input);
}

#[tokio::test]
async fn searches_across_resources() {
    let h = Harness::new().await;
    seed(&h).await;
    let found = call(
        &h,
        "workspace_search",
        json!({"query": "MIRRORLESS", "maxSnippetChars": 100}),
    )
    .await
    .unwrap();
    let types: Vec<&str> = found["matches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["resourceType"].as_str().unwrap())
        .collect();
    assert!(types.contains(&"artifact"));
    assert!(types.contains(&"message"));
    assert_eq!(found["total"], 2);
    assert_eq!(found["nextCursor"], Value::Null);
}

#[tokio::test]
async fn approves_rejects_and_resolves() {
    let h = Harness::new().await;
    let (action_id, papercut_id) = seed(&h).await;
    let actions = call(&h, "workspace_actions_list", json!({"status": "pending"}))
        .await
        .unwrap();
    assert_eq!(actions["total"], 1);
    let error = call(&h, "workspace_actions_list", json!({"subjectId": "camera"}))
        .await
        .unwrap_err();
    assert_eq!(
        error.message,
        "workspaceId is required when subjectId is provided"
    );

    let bad_id = call(
        &h,
        "workspace_action_approve",
        json!({"actionId": "not-a-uuid"}),
    )
    .await
    .unwrap_err();
    assert_eq!(bad_id.phase, ToolPhase::Input);
    let approved = call(
        &h,
        "workspace_action_approve",
        json!({"actionId": action_id}),
    )
    .await
    .unwrap();
    assert_eq!(approved["action"]["status"], "approved");
    assert_eq!(
        h.calendar.calls.lock().unwrap()[0].0,
        format!("workspace-{action_id}@omni-notify")
    );
    let again = call(
        &h,
        "workspace_action_reject",
        json!({"actionId": action_id}),
    )
    .await
    .unwrap_err();
    assert_eq!(again.message, "Workspace action is already approved");

    let papercuts = call(
        &h,
        "workspace_papercuts_list",
        json!({"workspaceId": "purchase-research"}),
    )
    .await
    .unwrap();
    assert_eq!(papercuts["papercuts"][0]["relatedTool"], Value::Null);
    let resolved = call(
        &h,
        "workspace_papercut_resolve",
        json!({"papercutId": papercut_id, "status": "dismissed", "resolution": "  Won't fix  "}),
    )
    .await
    .unwrap();
    assert_eq!(resolved["papercut"]["resolution"], "Won't fix");
    let missing = call(
        &h,
        "workspace_papercut_resolve",
        json!({"papercutId": "00000000-0000-4000-8000-000000000000", "status": "dismissed", "resolution": "x"}),
    )
    .await
    .unwrap_err();
    assert_eq!(
        missing.message,
        "Unknown workspace papercut \"00000000-0000-4000-8000-000000000000\""
    );
}

#[tokio::test]
async fn status_changes_keep_updated_at_and_messages_need_the_task() {
    let h = Harness::new().await;
    seed(&h).await;
    let changed = call(
        &h,
        "workspace_subject_set_status",
        json!({"workspaceId": "purchase-research", "subjectId": "camera", "status": "paused"}),
    )
    .await
    .unwrap();
    assert_eq!(changed["subject"]["status"], "paused");
    assert_eq!(changed["subject"]["updatedAt"], 1);

    let error = call(
        &h,
        "workspace_message",
        json!({"workspaceId": "purchase-research", "message": "Hello"}),
    )
    .await
    .unwrap_err();
    assert_eq!(error.message, "Unknown task \"PurchaseResearch\"");
}
