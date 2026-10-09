//! Port of `src/workspaces/notifications.test.ts` (fake outbox and notifier),
//! plus the outbox lifecycle against the real store.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use omni_store::StoreError;
use omni_store::cbor::Extra;
use omni_tasks::{RunContext, Task, Trigger};
use omni_workspaces::entities::{NotificationDraft, NotificationRow, NotificationStatus};
use omni_workspaces::{
    NotificationDelivery, NotificationOutbox, WorkspaceNotificationTask, WorkspaceNotifier,
};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct Log(Mutex<Vec<String>>);

impl Log {
    fn push(&self, entry: String) {
        self.0.lock().unwrap().push(entry);
    }
    fn entries(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
    fn count(&self, prefix: &str) -> usize {
        self.entries()
            .iter()
            .filter(|e| e.starts_with(prefix))
            .count()
    }
}

struct FakeOutbox {
    log: Arc<Log>,
    fail_sent_once: Mutex<bool>,
}

impl NotificationOutbox for FakeOutbox {
    fn mark_sending<'a>(
        &'a self,
        id: &'a str,
        attempts: i64,
    ) -> BoxFuture<'a, Result<(), StoreError>> {
        self.log.push(format!("sending {id} {attempts}"));
        Box::pin(async { Ok(()) })
    }
    fn mark_sent<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<(), StoreError>> {
        self.log.push(format!("sent {id}"));
        let fail = std::mem::take(&mut *self.fail_sent_once.lock().unwrap());
        Box::pin(async move {
            if fail {
                Err(StoreError::Sqlite("database unavailable".to_owned()))
            } else {
                Ok(())
            }
        })
    }
    fn mark_unknown<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<(), StoreError>> {
        self.log.push(format!("unknown {id}"));
        Box::pin(async { Ok(()) })
    }
    fn mark_failed<'a>(
        &'a self,
        id: &'a str,
        attempts: i64,
        error: String,
    ) -> BoxFuture<'a, Result<(), StoreError>> {
        self.log.push(format!("failed {id} {attempts} {error}"));
        Box::pin(async { Ok(()) })
    }
}

struct FakePushover {
    log: Arc<Log>,
    fail: Option<String>,
    gate: Option<(Arc<Notify>, Arc<Notify>)>,
}

impl WorkspaceNotifier for FakePushover {
    fn send<'a>(&'a self, n: &'a NotificationRow) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            self.log.push(format!("notify {}", n.notification_id));
            if let Some((entered, release)) = &self.gate {
                entered.notify_one();
                release.notified().await;
            }
            self.fail.clone().map_or(Ok(()), Err)
        })
    }
}

fn notification() -> NotificationRow {
    NotificationRow {
        notification_id: "notification-1".to_owned(),
        workspace_id: "purchase-research".to_owned(),
        subject_id: "camera".to_owned(),
        title: "Approval Needed".to_owned(),
        message: "Review the scope".to_owned(),
        url: "http://omni.boris/workspaces/purchase-research/camera?target=action-1".to_owned(),
        url_title: "Review Action".to_owned(),
        status: NotificationStatus::Pending,
        attempts: 0,
        created_at: 1,
        next_attempt_at: 1,
        sent_at: None,
        last_error: None,
        extra: Extra::new(),
    }
}

fn delivery(
    fail: Option<&str>,
    gate: Option<(Arc<Notify>, Arc<Notify>)>,
    fail_sent_once: bool,
) -> (Arc<Log>, Arc<NotificationDelivery>) {
    let log = Arc::new(Log::default());
    let outbox = Arc::new(FakeOutbox {
        log: log.clone(),
        fail_sent_once: Mutex::new(fail_sent_once),
    });
    let pushover = Arc::new(FakePushover {
        log: log.clone(),
        fail: fail.map(str::to_owned),
        gate,
    });
    (log, Arc::new(NotificationDelivery::new(outbox, pushover)))
}

#[tokio::test]
async fn records_success_after_pushover_accepts_the_notification() {
    let (log, delivery) = delivery(None, None, false);
    assert!(delivery.deliver(&notification()).await.unwrap());
    assert_eq!(
        log.entries(),
        [
            "sending notification-1 1",
            "notify notification-1",
            "sent notification-1"
        ]
    );
}

#[tokio::test]
async fn does_not_resend_after_delivery_succeeded_but_the_sent_acknowledgement_failed() {
    let (log, delivery) = delivery(None, None, true);
    let error = delivery.deliver(&notification()).await.unwrap_err();
    assert!(error.to_string().contains("database unavailable"));
    assert_eq!(log.count("notify"), 1);

    let reserved = NotificationRow {
        status: NotificationStatus::Sending,
        attempts: 1,
        ..notification()
    };
    assert!(delivery.deliver(&reserved).await.unwrap());
    assert_eq!(log.count("notify"), 1);
    assert_eq!(log.count("sent"), 1);
    assert!(log.entries().contains(&"unknown notification-1".to_owned()));
}

#[tokio::test]
async fn keeps_a_failed_delivery_queued_with_its_attempt_count() {
    let (log, delivery) = delivery(Some("Pushover unavailable"), None, false);
    assert!(!delivery.deliver(&notification()).await.unwrap());
    assert!(
        log.entries().contains(
            &"failed notification-1 1 send workspace notification failed: Pushover unavailable"
                .to_owned()
        )
    );
}

#[tokio::test]
async fn does_not_send_the_same_outbox_row_concurrently() {
    let (entered, release) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    let (log, delivery) = delivery(None, Some((entered.clone(), release.clone())), false);
    let first = {
        let delivery = delivery.clone();
        tokio::spawn(async move { delivery.deliver(&notification()).await })
    };
    entered.notified().await;
    assert!(!delivery.deliver(&notification()).await.unwrap());
    release.notify_one();
    assert!(first.await.unwrap().unwrap());
    assert_eq!(log.count("notify"), 1);
}

#[tokio::test]
async fn task_drains_the_store_outbox_and_backs_off_failures() {
    let h = common::Harness::new().await;
    let now = h.repo().now_ms();
    h.app
        .ctx
        .store
        .write(move |tx| {
            for id in ["a", "b"] {
                let row = NotificationDraft {
                    notification_id: id.to_owned(),
                    workspace_id: "purchase-research".to_owned(),
                    subject_id: "camera".to_owned(),
                    title: "T".to_owned(),
                    message: "M".to_owned(),
                    url: "http://omni.boris/x".to_owned(),
                    url_title: "Open".to_owned(),
                }
                .queue(now);
                omni_store::EntityWrite::upsert(tx, &row, Default::default())?;
            }
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
    *h.notifier.fail_with.lock().unwrap() = Some("Pushover unavailable".to_owned());
    let task = WorkspaceNotificationTask::new(
        h.repo().clone(),
        h.service.delivery().clone(),
        omni_tasks::CronSchedule::parse(
            WorkspaceNotificationTask::SCHEDULE,
            &jiff::tz::TimeZone::UTC,
        )
        .unwrap(),
    );
    let cx = RunContext {
        run_id: "WorkspaceNotifications:1".to_owned(),
        task_name: "WorkspaceNotifications".to_owned(),
        trigger: Trigger::Manual,
        scheduled_for: None,
        cancel: CancellationToken::new(),
    };
    task.run(&cx).await.unwrap();
    assert_eq!(
        task.last_run_summary().as_deref(),
        Some("Sent 0 notification(s); 2 queued for retry")
    );
    let failed: Vec<NotificationRow> = h
        .app
        .ctx
        .store
        .read(|d| omni_store::EntityOps::get_all::<NotificationRow>(d))
        .await
        .unwrap();
    for row in &failed {
        assert_eq!(row.status, NotificationStatus::Pending);
        assert_eq!(row.attempts, 1);
        // The test clock follows real time, so allow the run's own duration.
        assert!((now + 5 * 60_000..now + 5 * 60_000 + 5_000).contains(&row.next_attempt_at));
        assert_eq!(
            row.last_error.as_deref(),
            Some("send workspace notification failed: Pushover unavailable")
        );
    }
    // Not due yet: nothing is attempted.
    task.run(&cx).await.unwrap();
    assert_eq!(h.notifier.count(), 2);

    *h.notifier.fail_with.lock().unwrap() = None;
    h.app
        .ctx
        .store
        .write(move |tx| {
            for id in ["a", "b"] {
                omni_store::EntityWrite::update::<NotificationRow>(
                    tx,
                    &id.to_owned(),
                    |mut row| {
                        row.next_attempt_at = now;
                        row
                    },
                    Default::default(),
                )?;
            }
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
    task.run(&cx).await.unwrap();
    assert_eq!(
        task.last_run_summary().as_deref(),
        Some("Sent 2 notification(s); 0 queued for retry")
    );
    let sent: Vec<NotificationRow> = h
        .app
        .ctx
        .store
        .read(|d| omni_store::EntityOps::get_all::<NotificationRow>(d))
        .await
        .unwrap();
    assert!(sent.iter().all(|r| r.status == NotificationStatus::Sent
        && r.attempts == 2
        && r.last_error.is_none()
        && r.sent_at.is_some()));
}
