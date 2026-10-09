//! `workspace.updated`: one `action_pending` per created action and one
//! `reply_ready` per run, published after the output commits; a reprocessed
//! run and an identical pending proposal publish nothing new.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::Harness;
use omni_api::events::WORKSPACE_UPDATED;
use omni_api::workspaces::WorkspaceSubjectStatus;
use omni_testkit::RecordedEvents;
use omni_workspaces::engine::{OutputStatus, ProposalOutput, ProposalType, SubjectUpdate};
use omni_workspaces::entities::ActionRow;
use omni_workspaces::{RunRequest, RunTrigger, WorkspaceOutput};
use serde_json::json;

fn output() -> WorkspaceOutput {
    WorkspaceOutput {
        response: "Here is what I found.".to_owned(),
        subjects: vec![SubjectUpdate {
            subject_id: "camera".to_owned(),
            title: "Camera".to_owned(),
            status: OutputStatus::Active,
            summary: "Camera research".to_owned(),
            artifact_updates: vec![],
        }],
        sources: vec![],
        proposals: vec![ProposalOutput {
            proposal_type: ProposalType::EmailScope,
            subject_id: "camera".to_owned(),
            title: "Watch Sale Emails".to_owned(),
            description: "Watch one retailer.".to_owned(),
            senders: vec!["alerts@example.com".to_owned()],
            domains: vec![],
            subject_keywords: vec![],
            body_keywords: vec![],
            event: None,
        }],
        notification: None,
    }
}

fn request() -> RunRequest {
    RunRequest {
        trigger: RunTrigger::Message,
        message: Some("Find deals".to_owned()),
        subject_id: Some("camera".to_owned()),
    }
}

#[tokio::test]
async fn publishes_pending_actions_and_the_reply_once_per_run() {
    let h = Harness::new().await;
    let events = RecordedEvents::install(&h.app.ctx.ports);
    h.subject(
        "purchase-research",
        "camera",
        WorkspaceSubjectStatus::Active,
    )
    .await;
    let definition = h.service.definition("purchase-research").unwrap().clone();
    h.service
        .apply_output(&definition, output(), &request(), Some("run-1"), None)
        .await
        .unwrap();

    let published = events.published();
    assert_eq!(published.len(), 2);
    assert!(published.iter().all(|e| e.name == WORKSPACE_UPDATED));
    let action: ActionRow = h
        .app
        .ctx
        .store
        .read(|docs| omni_store::EntityOps::get_all::<ActionRow>(docs))
        .await
        .unwrap()
        .remove(0);
    let pending = &published[0];
    assert_eq!(pending.dedup_key, format!("{}:pending", action.action_id));
    assert_eq!(
        serde_json::Value::Object(pending.data.clone()),
        json!({
            "workspaceId": "purchase-research",
            "subjectId": "camera",
            "kind": "action_pending",
            "actionId": action.action_id,
            "actionType": "email_scope",
            "title": "Watch Sale Emails",
            "runId": "run-1",
        })
    );
    let reply = &published[1];
    assert_eq!(reply.dedup_key, "run:run-1:reply");
    assert_eq!(reply.data["kind"], "reply_ready");
    assert_eq!(reply.data["subjectId"], "camera");
    assert_eq!(reply.data["actionId"], json!(null));

    // Reprocessing the same run: the identical proposal is not recreated and
    // the reply keeps its dedup key.
    h.service
        .apply_output(&definition, output(), &request(), Some("run-1"), None)
        .await
        .unwrap();
    assert_eq!(events.published().len(), 3);
    assert_eq!(events.distinct().len(), 2);
}

#[tokio::test]
async fn an_unavailable_outbox_does_not_fail_the_run() {
    let h = Harness::new().await;
    let events = RecordedEvents::install(&h.app.ctx.ports);
    events.fail();
    h.subject(
        "purchase-research",
        "camera",
        WorkspaceSubjectStatus::Active,
    )
    .await;
    let definition = h.service.definition("purchase-research").unwrap().clone();
    let result = h
        .service
        .apply_output(&definition, output(), &request(), None, None)
        .await
        .unwrap();
    assert_eq!(result.created_actions, 1);
}
