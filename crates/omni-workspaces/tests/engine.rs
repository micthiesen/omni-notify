//! Port of `src/workspaces/engine.test.ts`, against a real store instead of a
//! mocked transaction (the plan-then-commit guarantee is checked on real rows).
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::Harness;
use omni_api::workspaces::{
    WorkspaceActionStatus, WorkspaceActionType, WorkspaceMessageRole, WorkspaceSubjectStatus,
};
use omni_workspaces::engine::{
    ArtifactUpdate, OutputStatus, ProposalOutput, ProposalType, SourceOutput, SubjectUpdate,
    normalize_web_url,
};
use omni_workspaces::entities::{
    ActionRow, ArtifactRevisionRow, MessageRow, NotificationRow, SourceRow, SubjectRow,
};
use omni_workspaces::persistence::{NewAction, NewMessage};
use omni_workspaces::{RunRequest, RunTrigger, WorkspaceOutput};

fn output(subject_id: &str) -> WorkspaceOutput {
    WorkspaceOutput {
        response: "I created the dossier.".to_owned(),
        subjects: vec![SubjectUpdate {
            subject_id: subject_id.to_owned(),
            title: "Camera".to_owned(),
            status: OutputStatus::Active,
            summary: "Camera research".to_owned(),
            artifact_updates: vec![ArtifactUpdate {
                key: "brief".to_owned(),
                content: "Requirements".to_owned(),
                summary: "Initial brief".to_owned(),
            }],
        }],
        sources: vec![SourceOutput {
            subject_id: subject_id.to_owned(),
            title: "Unsafe result".to_owned(),
            url: Some("javascript:alert(1)".to_owned()),
            excerpt: "Evidence".to_owned(),
        }],
        proposals: vec![ProposalOutput {
            proposal_type: ProposalType::EmailScope,
            subject_id: subject_id.to_owned(),
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

fn message_request(message: &str, subject_id: Option<&str>) -> RunRequest {
    RunRequest {
        trigger: RunTrigger::Message,
        message: Some(message.to_owned()),
        subject_id: subject_id.map(str::to_owned),
    }
}

async fn all<E: omni_store::Entity>(h: &Harness) -> Vec<E> {
    h.app
        .ctx
        .store
        .read(|docs| omni_store::EntityOps::get_all::<E>(docs))
        .await
        .unwrap()
}

fn definition(h: &Harness) -> omni_api::workspaces::WorkspaceDefinition {
    h.service.definition("purchase-research").unwrap().clone()
}

#[tokio::test]
async fn maps_one_new_subject_across_artifacts_sources_messages_and_actions() {
    let h = Harness::new().await;
    let result = h
        .service
        .apply_output(
            &definition(&h),
            output("new-1"),
            &message_request("Find me a camera", None),
            Some("PurchaseResearch:run-1"),
            None,
        )
        .await
        .unwrap();

    let subjects: Vec<SubjectRow> = all(&h).await;
    assert_eq!(subjects.len(), 1);
    let subject_id = subjects[0].subject_id.clone();
    assert_ne!(subject_id, "new-1");
    let artifacts: Vec<ArtifactRevisionRow> = all(&h).await;
    assert_eq!(artifacts[0].subject_id, subject_id);
    let sources: Vec<SourceRow> = all(&h).await;
    assert_eq!(sources[0].subject_id, subject_id);
    assert_eq!(sources[0].url, None);
    let actions: Vec<ActionRow> = all(&h).await;
    assert_eq!(actions[0].subject_id, subject_id);
    assert_eq!(actions[0].action_type, WorkspaceActionType::EmailScope);
    assert_eq!(actions[0].status, WorkspaceActionStatus::Pending);
    assert_eq!(
        actions[0].payload,
        r#"{"senders":["alerts@example.com"],"domains":[],"subjectKeywords":[],"bodyKeywords":[]}"#
    );
    let messages: Vec<MessageRow> = all(&h).await;
    assert_eq!(messages.len(), 2);
    assert!(
        messages
            .iter()
            .all(|m| m.subject_id.as_deref() == Some(subject_id.as_str()))
    );
    assert_eq!(result.updated_subjects, 1);
    assert_eq!(result.created_actions, 1);
    assert_eq!(h.notifier.count(), 1);
    let sent = h.notifier.sent.lock().unwrap()[0].clone();
    assert_eq!(
        sent.notification_id,
        format!("action:{}", actions[0].action_id)
    );
    assert_eq!(sent.title, "Approval Needed: Watch Sale Emails");
    assert_eq!(
        sent.url,
        format!(
            "http://omni.boris/workspaces/purchase-research/{subject_id}?section=actions&target=action-{}",
            actions[0].action_id
        )
    );
    let outbox: Vec<NotificationRow> = all(&h).await;
    assert_eq!(outbox[0].attempts, 1);
}

#[tokio::test]
async fn attaches_a_pre_model_user_message_to_the_subject_without_duplicating_it() {
    let h = Harness::new().await;
    let persisted = h
        .repo()
        .add_message(NewMessage {
            workspace_id: "purchase-research".to_owned(),
            subject_id: None,
            role: WorkspaceMessageRole::User,
            text: "Find me a camera".to_owned(),
            run_id: Some("run-1".to_owned()),
        })
        .await
        .unwrap();
    h.service
        .apply_output(
            &definition(&h),
            output("new-1"),
            &message_request("Find me a camera", None),
            Some("run-1"),
            Some(persisted.message_id.clone()),
        )
        .await
        .unwrap();
    let subject_id = all::<SubjectRow>(&h).await[0].subject_id.clone();
    let messages: Vec<MessageRow> = all(&h).await;
    assert_eq!(messages.len(), 2);
    let user = messages
        .iter()
        .find(|m| m.message_id == persisted.message_id)
        .unwrap();
    assert_eq!(user.subject_id.as_deref(), Some(subject_id.as_str()));
    assert_eq!(
        messages
            .iter()
            .filter(|m| m.role == WorkspaceMessageRole::User)
            .count(),
        1
    );
}

#[tokio::test]
async fn rejects_hallucinated_subject_ids_instead_of_allocating_a_dossier() {
    let h = Harness::new().await;
    let error = h
        .service
        .apply_output(
            &definition(&h),
            output("typo-subject"),
            &message_request("Update it", None),
            None,
            None,
        )
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("unknown subject_id \"typo-subject\"")
    );
    assert_eq!(h.count::<SubjectRow>().await, 0);
}

#[tokio::test]
async fn does_not_let_a_subject_scoped_run_update_another_dossier() {
    let h = Harness::new().await;
    h.subject(
        "purchase-research",
        "camera",
        WorkspaceSubjectStatus::Active,
    )
    .await;
    let error = h
        .service
        .apply_output(
            &definition(&h),
            output("new-1"),
            &message_request("Update it", Some("camera")),
            None,
            None,
        )
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Subject-scoped run attempted to update")
    );
}

#[tokio::test]
async fn validates_late_references_before_writing_any_part_of_the_output() {
    let h = Harness::new().await;
    let mut invalid = output("new-1");
    invalid.sources.push(SourceOutput {
        subject_id: "missing-subject".to_owned(),
        title: "Late invalid source".to_owned(),
        url: Some("https://example.com".to_owned()),
        excerpt: "Must invalidate the complete plan".to_owned(),
    });
    let error = h
        .service
        .apply_output(
            &definition(&h),
            invalid,
            &message_request("Find me a camera", None),
            None,
            None,
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("missing-subject"));
    assert_eq!(h.count::<SubjectRow>().await, 0);
    assert_eq!(h.count::<ArtifactRevisionRow>().await, 0);
    assert_eq!(h.count::<SourceRow>().await, 0);
    assert_eq!(h.count::<ActionRow>().await, 0);
    assert_eq!(h.count::<MessageRow>().await, 0);
    assert_eq!(h.count::<NotificationRow>().await, 0);
    assert_eq!(h.notifier.count(), 0);
}

#[tokio::test]
async fn does_not_renotify_an_existing_pending_action() {
    let h = Harness::new().await;
    h.subject(
        "purchase-research",
        "camera",
        WorkspaceSubjectStatus::Active,
    )
    .await;
    h.repo()
        .add_action(NewAction {
            workspace_id: "purchase-research".to_owned(),
            subject_id: "camera".to_owned(),
            action_type: WorkspaceActionType::EmailScope,
            title: "Watch Sale Emails".to_owned(),
            description: "Watch one retailer.".to_owned(),
            payload: serde_json::json!({
                "senders": ["alerts@example.com"],
                "domains": [],
                "subjectKeywords": [],
                "bodyKeywords": [],
            })
            .to_string(),
            run_id: None,
        })
        .await
        .unwrap();
    let result = h
        .service
        .apply_output(
            &definition(&h),
            output("camera"),
            &message_request("Check again", Some("camera")),
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(result.created_actions, 0);
    assert_eq!(h.count::<ActionRow>().await, 1);
    assert_eq!(h.notifier.count(), 0);
}

#[tokio::test]
async fn sends_an_update_notification_only_without_new_actions() {
    let h = Harness::new().await;
    h.subject(
        "purchase-research",
        "camera",
        WorkspaceSubjectStatus::Active,
    )
    .await;
    let mut update = output("camera");
    update.proposals.clear();
    update.notification = Some(omni_workspaces::engine::NotificationOutput {
        subject_id: "camera".to_owned(),
        title: "Price drop".to_owned(),
        message: "Now $499".to_owned(),
        artifact_key: Some("comparison".to_owned()),
    });
    h.service
        .apply_output(
            &definition(&h),
            update,
            &RunRequest {
                trigger: RunTrigger::Scheduled,
                message: None,
                subject_id: Some("camera".to_owned()),
            },
            Some("PurchaseResearch:abc"),
            None,
        )
        .await
        .unwrap();
    let sent = h.notifier.sent.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].notification_id, "update:PurchaseResearch:abc");
    assert_eq!(sent[0].url_title, "Open Purchase");
    assert!(sent[0].url.ends_with(
        "/workspaces/purchase-research/camera?section=artifacts&target=artifact-comparison"
    ));
    let subject: SubjectRow = all::<SubjectRow>(&h).await.remove(0);
    assert!(
        subject.last_researched_at.is_some(),
        "scheduled runs stamp lastResearchedAt"
    );
    let messages: Vec<MessageRow> = all(&h).await;
    assert_eq!(
        messages.len(),
        1,
        "a scheduled run records only the assistant reply"
    );
}

#[tokio::test]
async fn unchanged_artifact_content_writes_no_new_revision() {
    let h = Harness::new().await;
    h.subject(
        "purchase-research",
        "camera",
        WorkspaceSubjectStatus::Active,
    )
    .await;
    let mut update = output("camera");
    update.proposals.clear();
    update.sources.clear();
    let request = message_request("Again", Some("camera"));
    h.service
        .apply_output(&definition(&h), update.clone(), &request, None, None)
        .await
        .unwrap();
    update.subjects[0].artifact_updates[0].content = "  Requirements \n".to_owned();
    h.service
        .apply_output(&definition(&h), update, &request, None, None)
        .await
        .unwrap();
    assert_eq!(h.count::<ArtifactRevisionRow>().await, 1);
}

#[test]
fn allows_only_http_web_links() {
    assert_eq!(
        normalize_web_url(Some("https://example.com/deal")).as_deref(),
        Some("https://example.com/deal")
    );
    assert_eq!(
        normalize_web_url(Some("http://example.com/deal")).as_deref(),
        Some("http://example.com/deal")
    );
    assert_eq!(normalize_web_url(Some("javascript:alert(1)")), None);
    assert_eq!(normalize_web_url(Some("data:text/html,bad")), None);
    assert_eq!(normalize_web_url(Some("not a URL")), None);
}
