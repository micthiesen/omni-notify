//! Workspace action proposals and approvals against a real store and a fake
//! `CalendarWriter` port.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::sync::Arc;

use common::Harness;
use omni_api::workspaces::{WorkspaceActionStatus, WorkspaceActionType};
use omni_runtime::ports::{CalendarCreateOutcome, PortError};
use omni_workspaces::persistence::NewAction;
use serde_json::{Value, json};
use tokio::sync::Notify;

async fn action(h: &Harness, action_type: WorkspaceActionType, payload: Value) -> String {
    let (row, created) = h
        .repo()
        .add_action(NewAction {
            workspace_id: "purchase-research".to_owned(),
            subject_id: "subject-1".to_owned(),
            action_type,
            title: "Test action".to_owned(),
            description: "Test action description".to_owned(),
            payload: payload.to_string(),
            run_id: None,
        })
        .await
        .unwrap();
    assert!(created);
    row.action_id
}

fn deadline() -> Value {
    json!({ "title": "Return deadline", "startDate": "2026-09-01", "allDay": true })
}

#[tokio::test]
async fn enables_a_validated_email_scope_only_after_approval() {
    let h = Harness::new().await;
    let id = action(
        &h,
        WorkspaceActionType::EmailScope,
        json!({ "senders": ["orders@example.com"], "domains": [], "subjectKeywords": [], "bodyKeywords": [] }),
    )
    .await;
    assert!(
        h.repo()
            .get_email_scope("purchase-research", "subject-1")
            .await
            .unwrap()
            .is_none()
    );

    let approved = h.service.approve_action(&id).await.unwrap();

    assert_eq!(approved.status, WorkspaceActionStatus::Approved);
    assert_eq!(approved.result.as_deref(), Some("Email scope enabled"));
    let scope = h
        .repo()
        .get_email_scope("purchase-research", "subject-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(scope.senders, ["orders@example.com"]);
}

#[tokio::test]
async fn uses_a_deterministic_calendar_uid_and_treats_an_existing_put_as_success() {
    let h = Harness::new().await;
    let id = action(&h, WorkspaceActionType::CalendarEvent, deadline()).await;
    h.calendar
        .outcomes
        .lock()
        .unwrap()
        .push_back(Ok(CalendarCreateOutcome::AlreadyExists));

    let approved = h.service.approve_action(&id).await.unwrap();

    assert_eq!(approved.status, WorkspaceActionStatus::Approved);
    assert_eq!(
        approved.result.as_deref(),
        Some("Calendar event was already created")
    );
    let calls = h.calendar.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, format!("workspace-{id}@omni-notify"));
    assert_eq!(calls[0].1.title, "Return deadline");
    assert!(calls[0].1.all_day);
}

#[tokio::test]
async fn allows_retrying_a_transiently_failed_calendar_approval() {
    let h = Harness::new().await;
    let id = action(&h, WorkspaceActionType::CalendarEvent, deadline()).await;
    h.calendar.outcomes.lock().unwrap().extend([
        Err(PortError::Failed {
            message: "Unavailable".to_owned(),
            transient: true,
        }),
        Ok(CalendarCreateOutcome::Created {
            event_uid: "workspace-action-1".to_owned(),
        }),
    ]);

    let failure = h.service.approve_action(&id).await.unwrap_err();
    assert!(failure.to_string().contains("Unavailable"));
    let stored = h.repo().get_action(&id).await.unwrap().unwrap();
    assert_eq!(stored.status, WorkspaceActionStatus::Failed);
    let approved = h.service.approve_action(&id).await.unwrap();
    assert_eq!(approved.status, WorkspaceActionStatus::Approved);
    assert_eq!(
        approved.result.as_deref(),
        Some("Calendar event created (workspace-action-1)")
    );
}

#[tokio::test]
async fn blocks_rejection_while_an_approval_side_effect_is_in_flight() {
    let h = Harness::new().await;
    let id = action(&h, WorkspaceActionType::CalendarEvent, deadline()).await;
    let (entered, release) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    *h.calendar.gate.lock().unwrap() = Some((entered.clone(), release.clone()));

    let service = h.service.clone();
    let approving = id.clone();
    let approval = tokio::spawn(async move { service.approve_action(&approving).await });
    entered.notified().await;

    let rejection = h.service.reject_action(&id).await.unwrap_err();
    assert!(rejection.to_string().contains("already being resolved"));
    release.notify_one();
    let approved = approval.await.unwrap().unwrap();
    assert_eq!(approved.status, WorkspaceActionStatus::Approved);
}

#[tokio::test]
async fn rejection_requires_a_pending_action_and_an_unbounded_scope_fails() {
    let h = Harness::new().await;
    assert_eq!(
        h.service
            .reject_action("missing")
            .await
            .unwrap_err()
            .to_string(),
        "Workspace action not found"
    );
    let id = action(
        &h,
        WorkspaceActionType::EmailScope,
        json!({ "senders": [" a "], "domains": [], "subjectKeywords": [], "bodyKeywords": [] }),
    )
    .await;
    let error = h.service.approve_action(&id).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "An email scope must contain bounded, non-empty matchers"
    );
    let failed = h.repo().get_action(&id).await.unwrap().unwrap();
    assert_eq!(failed.status, WorkspaceActionStatus::Failed);
    assert_eq!(
        h.service.reject_action(&id).await.unwrap_err().to_string(),
        "Workspace action is already failed"
    );
    let pending = action(&h, WorkspaceActionType::CalendarEvent, deadline()).await;
    let rejected = h.service.reject_action(&pending).await.unwrap();
    assert_eq!(rejected.status, WorkspaceActionStatus::Rejected);
    assert_eq!(rejected.result.as_deref(), Some("Rejected by user"));
    assert_eq!(
        h.service
            .approve_action(&pending)
            .await
            .unwrap_err()
            .to_string(),
        "Workspace action is already rejected"
    );
    assert!(
        h.calendar.calls.lock().unwrap().is_empty(),
        "nothing reached CalDAV"
    );
}

#[tokio::test]
async fn invalid_payload_json_marks_the_action_failed() {
    let h = Harness::new().await;
    let (row, _) = h
        .repo()
        .add_action(NewAction {
            workspace_id: "purchase-research".to_owned(),
            subject_id: "subject-1".to_owned(),
            action_type: WorkspaceActionType::CalendarEvent,
            title: "Broken".to_owned(),
            description: "Broken".to_owned(),
            payload: "{not json".to_owned(),
            run_id: None,
        })
        .await
        .unwrap();
    let error = h.service.approve_action(&row.action_id).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("Workspace action {} has invalid JSON", row.action_id)
    );
    let stored = h.repo().get_action(&row.action_id).await.unwrap().unwrap();
    assert_eq!(stored.status, WorkspaceActionStatus::Failed);
}
