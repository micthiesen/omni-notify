//! Port of `src/alerts/castro.spec.ts` (the Castro persistent-failure gate),
//! plus an end-to-end check through the foundation alert layer.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, RwLock};

use omni_alerts::{AlertGate, AlertLayer};
use omni_podcasts::castro::alert_gate::{CastroFailureGate, has_persistent_castro_failure};
use omni_store::cbor::Extra;
use omni_store::entity::{EntityWrite as _, UpsertOpts};
use omni_tasks::{TaskRunData, TaskRunStatus, Trigger};
use omni_testkit::{TestApp, TestStore, test_clock};
use tracing_subscriber::layer::SubscriberExt as _;

const HOUR: f64 = 60.0 * 60_000.0;

fn run(hours: f64, status: TaskRunStatus) -> TaskRunData {
    TaskRunData {
        run_id: format!("castro-{hours}"),
        task_name: "CastroInboxCleanup".into(),
        trigger: Trigger::Schedule,
        scheduled_for: None,
        started_at: (hours * HOUR) as i64,
        finished_at: None,
        status,
        error: None,
        summary: None,
        extra: Extra::default(),
    }
}

fn err(hours: f64) -> TaskRunData {
    run(hours, TaskRunStatus::Error)
}

#[test]
fn requires_three_consecutive_failures_spanning_twelve_hours_allowing_jitter() {
    assert!(!has_persistent_castro_failure(&[err(12.0)]));
    assert!(!has_persistent_castro_failure(&[err(12.0), err(0.0)]));
    assert!(!has_persistent_castro_failure(&[
        err(2.0),
        err(1.0),
        err(0.0)
    ]));
    assert!(has_persistent_castro_failure(&[
        err(12.0),
        err(6.0),
        err(0.0)
    ]));
    assert!(has_persistent_castro_failure(&[
        err(12.0),
        err(6.0),
        err(0.08)
    ]));
    assert!(!has_persistent_castro_failure(&[
        err(12.0),
        run(6.0, TaskRunStatus::Success),
        err(0.0)
    ]));
    assert!(!has_persistent_castro_failure(&[
        run(18.0, TaskRunStatus::Running),
        err(12.0),
        err(6.0),
        err(0.0)
    ]));
}

async fn insert(store: &omni_store::Store, data: TaskRunData) {
    store
        .write(move |tx| tx.upsert(&data, UpsertOpts::default()))
        .await
        .unwrap();
}

/// Delivered when no gate applies or the gate admits it.
async fn delivered(title: &str, store: &omni_store::Store) -> bool {
    // Construct afresh to model a service restart: no in-memory streak.
    let gate = CastroFailureGate::new(store.clone());
    !gate.applies(title) || gate.should_notify(title).await
}

#[tokio::test]
async fn uses_persisted_history_across_fresh_hooks_for_scheduled_manual_and_catch_up_failures() {
    let store = TestStore::new(test_clock(0)).await;
    let store = &store.store;
    let scheduled = "Error running task \"CastroInboxCleanup\"";
    let mut count = 0;
    insert(store, err(0.0)).await;
    count += usize::from(delivered(scheduled, store).await);
    insert(store, err(6.0)).await;
    count += usize::from(delivered(scheduled, store).await);
    assert_eq!(count, 0);
    insert(store, err(12.0)).await;
    count += usize::from(delivered(scheduled, store).await);
    count += usize::from(delivered("Manual run of \"CastroInboxCleanup\" failed", store).await);
    count += usize::from(delivered("Catch-up run of \"CastroInboxCleanup\" failed", store).await);
    assert_eq!(count, 3);
    insert(store, run(18.0, TaskRunStatus::Success)).await;
    insert(store, err(24.0)).await;
    count += usize::from(delivered(scheduled, store).await);
    assert_eq!(count, 3);
    count += usize::from(delivered("Error running task \"OtherTask\"", store).await);
    assert_eq!(count, 4);
}

#[tokio::test]
async fn alert_layer_withholds_castro_alerts_until_the_failure_persists() {
    let app = TestApp::new().await;
    let store = app.ctx.store.clone();
    let gate: Arc<dyn AlertGate> = Arc::new(CastroFailureGate::new(store.clone()));
    let emit = |message: &'static str| {
        let (layer, worker) = AlertLayer::new(
            app.ctx.pushover.clone(),
            Arc::new(RwLock::new(vec![gate.clone()])),
            app.ctx.clock.clone(),
        );
        let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            tracing::error!(target: "Scheduler", error = "socket hang up", "{message}");
        });
        worker.run()
    };
    insert(&store, err(0.0)).await;
    insert(&store, err(6.0)).await;
    emit("Error running task \"CastroInboxCleanup\"").await;
    assert!(app.pushes.all().is_empty(), "two failures must not notify");
    insert(&store, err(12.0)).await;
    emit("Error running task \"CastroInboxCleanup\"").await;
    let pushes = app.pushes.all();
    assert_eq!(pushes.len(), 1);
    assert_eq!(
        pushes[0].message.title.as_deref(),
        Some("Error: Error running task \"CastroInboxCleanup\"")
    );
}

#[tokio::test]
async fn gates_isolated_inbox_clear_failures_logged_by_the_client() {
    let store = TestStore::new(test_clock(0)).await;
    let store = &store.store;
    let title = omni_podcasts::castro::alert_gate::INBOX_CLEAR_FAILED_TITLE;
    // A single 500 during cleanup: the run is still in progress.
    insert(store, run(0.0, TaskRunStatus::Running)).await;
    assert!(!delivered(title, store).await);
    // Even after a persistent streak, the in-progress run heads the history,
    // so the run's own failure alert is the one that notifies.
    insert(store, err(-12.0)).await;
    insert(store, err(-6.0)).await;
    insert(store, err(-1.0)).await;
    assert!(!delivered(title, store).await);
}
