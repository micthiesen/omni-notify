//! The workspace task, driving the real engine with a
//! scripted workspace model.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::Harness;
use omni_ai::{GenerateResponse, ModelRole};
use omni_api::workspaces::WorkspaceSubjectStatus;
use omni_tasks::{CronSchedule, Task};
use omni_workspaces::WorkspaceTask;
use serde_json::json;

fn task(h: &Harness) -> WorkspaceTask {
    let definition = h.service.definition("marketplace-selling").unwrap().clone();
    let schedule = CronSchedule::parse(&definition.schedule, &jiff::tz::TimeZone::UTC).unwrap();
    WorkspaceTask::new(h.service.clone(), definition, schedule)
}

#[tokio::test]
async fn skips_scheduled_work_even_when_an_active_subject_exists() {
    let h = Harness::new().await;
    h.subject(
        "marketplace-selling",
        "desk",
        WorkspaceSubjectStatus::Active,
    )
    .await;
    let task = task(&h);
    task.run_scheduled(Some("MarketplaceSelling:1"))
        .await
        .unwrap();
    assert!(h.app.ai.requests().is_empty());
    assert_eq!(
        task.last_run_summary().as_deref(),
        Some("On-demand workspace; scheduled refresh skipped")
    );
}

#[tokio::test]
async fn still_responds_to_a_manual_message() {
    let h = Harness::new().await;
    let reply = json!({
        "response": "Updated the listing draft",
        "subjects": [{
            "subject_id": "new-desk",
            "title": "Standing Desk",
            "status": "active",
            "summary": "Drafting the listing",
            "artifact_updates": [{"key": "item-details", "content": "60 inches wide", "summary": "Width"}]
        }],
        "sources": [],
        "proposals": [],
        "notification": null
    });
    h.app.ai.script(
        ModelRole::Workspace,
        vec![GenerateResponse::text(reply.to_string())],
    );
    let task = task(&h);
    task.run_with_input(
        json!({ "message": "The desk is 60 inches wide" }),
        Some("MarketplaceSelling:2"),
    )
    .await
    .unwrap();

    let requests = h.app.ai.requests();
    assert_eq!(requests.len(), 1);
    let (role, request) = &requests[0];
    assert_eq!(*role, Some(ModelRole::Workspace));
    let prompt = serde_json::to_string(&request.messages).unwrap();
    assert!(prompt.contains("Trigger: message"));
    assert!(prompt.contains("Requested subject: none"));
    assert!(prompt.contains("User/input message: The desk is 60 inches wide"));
    assert!(request.output.is_some());
    let tools: Vec<&str> = request.tools.iter().map(|t| t.name.as_str()).collect();
    assert!(tools.contains(&"report_papercut"));
    assert_eq!(
        task.last_run_summary().as_deref(),
        Some("Updated the listing draft")
    );
    let subjects = h.repo().list_subjects("marketplace-selling").await.unwrap();
    assert_eq!(subjects.len(), 1);
    let messages = h
        .repo()
        .list_messages("marketplace-selling", Some(&subjects[0].subject_id), 10)
        .await
        .unwrap();
    assert_eq!(
        messages.len(),
        2,
        "the pre-model user message is attached, not duplicated"
    );
}

#[tokio::test]
async fn rejects_empty_manual_input_and_keeps_the_user_message_when_the_model_fails() {
    let h = Harness::new().await;
    let task = task(&h);
    let error = task
        .run_with_input(json!({ "message": "   " }), None)
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Workspace manual input is empty or too long"
    );
    let error = task
        .run_with_input(json!({ "nope": 1 }), None)
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "Invalid workspace manual input");
    // No scripted response: the model call fails after the message was stored.
    assert!(
        task.run_with_input(json!({ "message": "Sell my bike" }), None)
            .await
            .is_err()
    );
    let messages = h
        .repo()
        .list_messages("marketplace-selling", None, 10)
        .await
        .unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].text, "Sell my bike");
}
