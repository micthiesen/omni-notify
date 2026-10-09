//! Shared fixtures: a `TestApp`-backed service with recording fakes.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use omni_api::workspaces::WorkspaceSubjectStatus;
use omni_core::email::FetchedEmail;
use omni_runtime::ports::{
    CalendarCreateOutcome, CalendarEventInput, CalendarWriter, CalendarWriterStatus, PortError,
};
use omni_testkit::{FakeTool, TestApp};
use omni_workspaces::entities::{NotificationRow, SubjectRow};
use omni_workspaces::persistence::SubjectUpsert;
use omni_workspaces::{WorkspaceDeps, WorkspaceNotifier, WorkspaceRepo, WorkspaceService};
use tokio::sync::Notify;

/// Records every push; optionally fails or waits for a release.
#[derive(Default)]
pub struct RecordingNotifier {
    pub sent: Mutex<Vec<NotificationRow>>,
    pub fail_with: Mutex<Option<String>>,
    /// When set, `send` signals `entered` and waits for `release`.
    pub gate: Mutex<Option<(Arc<Notify>, Arc<Notify>)>>,
}

impl RecordingNotifier {
    pub fn count(&self) -> usize {
        self.sent.lock().unwrap().len()
    }
}

impl WorkspaceNotifier for RecordingNotifier {
    fn send<'a>(&'a self, n: &'a NotificationRow) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            self.sent.lock().unwrap().push(n.clone());
            let gate = self.gate.lock().unwrap().clone();
            if let Some((entered, release)) = gate {
                entered.notify_one();
                release.notified().await;
            }
            match self.fail_with.lock().unwrap().clone() {
                Some(message) => Err(message),
                None => Ok(()),
            }
        })
    }
}

/// A scripted `CalendarWriter`.
#[derive(Default)]
pub struct FakeCalendar {
    pub calls: Mutex<Vec<(String, CalendarEventInput)>>,
    pub outcomes: Mutex<VecDeque<Result<CalendarCreateOutcome, PortError>>>,
    pub gate: Mutex<Option<(Arc<Notify>, Arc<Notify>)>>,
}

impl CalendarWriter for FakeCalendar {
    fn create_event<'a>(
        &'a self,
        uid: &'a str,
        input: &'a CalendarEventInput,
    ) -> BoxFuture<'a, Result<CalendarCreateOutcome, PortError>> {
        Box::pin(async move {
            self.calls
                .lock()
                .unwrap()
                .push((uid.to_owned(), input.clone()));
            let gate = self.gate.lock().unwrap().clone();
            if let Some((entered, release)) = gate {
                entered.notify_one();
                release.notified().await;
            }
            self.outcomes
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| {
                    Ok(CalendarCreateOutcome::Created {
                        event_uid: uid.to_owned(),
                    })
                })
        })
    }

    fn status(&self) -> CalendarWriterStatus {
        CalendarWriterStatus {
            configured: true,
            provider: Some("fake".to_owned()),
        }
    }
}

pub struct Harness {
    pub app: TestApp,
    pub service: WorkspaceService,
    pub notifier: Arc<RecordingNotifier>,
    pub calendar: Arc<FakeCalendar>,
}

impl Harness {
    pub async fn new() -> Self {
        let app = TestApp::new().await;
        let notifier = Arc::new(RecordingNotifier::default());
        let calendar = Arc::new(FakeCalendar::default());
        app.ctx.ports.set_calendar_writer(calendar.clone()).unwrap();
        let service = WorkspaceService::new(WorkspaceDeps {
            repo: WorkspaceRepo::new(app.ctx.store.clone()),
            config: app.ctx.config.clone(),
            ai: app.ctx.ai.clone(),
            web_search: Arc::new(FakeTool::new("web_search", vec![])),
            fetch_url: Arc::new(FakeTool::new("fetch_url", vec![])),
            notifier: notifier.clone(),
            outbox: None,
            ports: app.ctx.ports.clone(),
            tasks: app.ctx.tasks.clone(),
            tracker: app.ctx.tracker.clone(),
        });
        Self {
            app,
            service,
            notifier,
            calendar,
        }
    }

    pub fn repo(&self) -> &WorkspaceRepo {
        self.service.repo()
    }

    pub async fn subject(
        &self,
        workspace_id: &str,
        subject_id: &str,
        status: WorkspaceSubjectStatus,
    ) -> SubjectRow {
        self.repo()
            .upsert_subject(SubjectUpsert {
                workspace_id: workspace_id.to_owned(),
                subject_id: subject_id.to_owned(),
                title: "Camera".to_owned(),
                status,
                summary: "Camera research".to_owned(),
                created_at: Some(1),
                updated_at: Some(1),
                last_researched_at: None,
            })
            .await
            .unwrap()
    }

    pub async fn count<E: omni_store::Entity>(&self) -> u64 {
        self.app
            .ctx
            .store
            .read(|docs| omni_store::EntityOps::count::<E>(docs))
            .await
            .unwrap()
    }
}

pub fn email(id: &str, subject: &str, from: &str, body: &str) -> FetchedEmail {
    serde_json::from_value(serde_json::json!({
        "id": id,
        "subject": subject,
        "from": from,
        "textBody": body,
        "links": [],
        "receivedAt": "2026-08-18T10:00:00.000Z",
        "attachments": [],
    }))
    .unwrap()
}
