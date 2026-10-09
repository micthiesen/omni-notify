//! Port of `src/workspaces/emailHandler.test.ts` against a real store and a
//! fake run trigger.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use common::{Harness, email};
use futures::future::BoxFuture;
use omni_api::workspaces::{WorkspaceEmailScope, WorkspaceSubjectStatus};
use omni_workspaces::entities::SourceRow;
use omni_workspaces::{EmailRunTrigger, WorkspaceEmailHandler, WorkspaceError};
use tokio::sync::Notify;

#[derive(Default)]
struct FakeTrigger {
    calls: Mutex<Vec<(String, String, String)>>,
    failures: Mutex<VecDeque<String>>,
    gate: Mutex<Option<(Arc<Notify>, Arc<Notify>)>>,
}

impl EmailRunTrigger for FakeTrigger {
    fn trigger<'a>(
        &'a self,
        workspace_id: &'a str,
        subject_id: &'a str,
        message: String,
    ) -> BoxFuture<'a, Result<(), WorkspaceError>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push((
                workspace_id.to_owned(),
                subject_id.to_owned(),
                message,
            ));
            let gate = self.gate.lock().unwrap().clone();
            if let Some((entered, release)) = gate {
                entered.notify_one();
                release.notified().await;
            }
            match self.failures.lock().unwrap().pop_front() {
                Some(message) => Err(WorkspaceError::operation("workspace run", message)),
                None => Ok(()),
            }
        })
    }
}

async fn setup() -> (Harness, Arc<FakeTrigger>, Arc<WorkspaceEmailHandler>) {
    let h = Harness::new().await;
    h.subject(
        "purchase-research",
        "camera",
        WorkspaceSubjectStatus::Active,
    )
    .await;
    h.repo()
        .upsert_email_scope(
            "purchase-research",
            "camera",
            WorkspaceEmailScope {
                senders: vec!["alerts@shop.example".to_owned()],
                ..WorkspaceEmailScope::default()
            },
        )
        .await
        .unwrap();
    let trigger = Arc::new(FakeTrigger::default());
    let handler = Arc::new(WorkspaceEmailHandler::new(
        h.repo().clone(),
        trigger.clone(),
    ));
    (h, trigger, handler)
}

fn mail() -> omni_core::email::FetchedEmail {
    email(
        "email-1",
        "Camera price drop",
        "alerts@shop.example",
        "The camera is now on sale.",
    )
}

async fn sources(h: &Harness) -> Vec<SourceRow> {
    h.app
        .ctx
        .store
        .read(|docs| omni_store::EntityOps::get_all::<SourceRow>(docs))
        .await
        .unwrap()
}

#[tokio::test]
async fn persists_and_triggers_a_newly_matched_active_subject_email_once() {
    let (h, trigger, handler) = setup().await;
    handler.handle_emails(&[mail()]).await.unwrap();
    handler.handle_emails(&[mail()]).await.unwrap();

    let stored = sources(&h).await;
    assert_eq!(stored.len(), 1);
    assert_eq!(
        stored[0].source_id,
        "email:purchase-research:camera:email-1"
    );
    assert_eq!(stored[0].email_id.as_deref(), Some("email-1"));
    let calls = trigger.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "purchase-research");
    assert_eq!(calls[0].1, "camera");
    assert!(calls[0].2.contains("Camera price drop"));
    assert_eq!(
        calls[0].2,
        "Review 1 newly ingested scoped email(s): Camera price drop"
    );
}

#[tokio::test]
async fn does_not_ingest_for_a_paused_subject() {
    let (h, trigger, handler) = setup().await;
    h.subject(
        "purchase-research",
        "camera",
        WorkspaceSubjectStatus::Paused,
    )
    .await;
    handler.handle_emails(&[mail()]).await.unwrap();
    assert!(sources(&h).await.is_empty());
    assert!(trigger.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn retries_a_persisted_email_whose_workspace_trigger_failed() {
    let (h, trigger, handler) = setup().await;
    trigger
        .failures
        .lock()
        .unwrap()
        .push_back("Workspace run failed".to_owned());

    let error = handler.handle_emails(&[mail()]).await.unwrap_err();
    assert!(error.to_string().contains("Workspace run failed"));
    let stored = sources(&h).await;
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].triggered_at, None);

    handler.handle_emails(&[mail()]).await.unwrap();
    assert_eq!(trigger.calls.lock().unwrap().len(), 2);
    assert!(sources(&h).await[0].triggered_at.is_some());
}

#[tokio::test]
async fn does_not_mark_sources_triggered_until_the_workspace_run_completes() {
    let (h, trigger, handler) = setup().await;
    let (entered, release) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    *trigger.gate.lock().unwrap() = Some((entered.clone(), release.clone()));
    let running = handler.clone();
    let handling = tokio::spawn(async move { running.handle_emails(&[mail()]).await });
    entered.notified().await;
    assert_eq!(trigger.calls.lock().unwrap().len(), 1);
    assert_eq!(sources(&h).await[0].triggered_at, None);

    release.notify_one();
    handling.await.unwrap().unwrap();
    assert!(sources(&h).await[0].triggered_at.is_some());
}

#[tokio::test]
async fn handler_failures_are_transient_for_replay() {
    use omni_core::email::EmailHandler as _;
    let (_h, trigger, handler) = setup().await;
    trigger
        .failures
        .lock()
        .unwrap()
        .push_back("boom".to_owned());
    assert_eq!(handler.name(), "Workspaces");
    let error = handler.handle(&[mail()]).await.unwrap_err();
    assert!(error.transient);
    assert_eq!(error.message, "workspace run failed: boom");
}
