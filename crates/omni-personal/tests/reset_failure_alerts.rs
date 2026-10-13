//! Reset tracker read failures reach Pushover only after thirty minutes of
//! consecutive failed runs, judged from durable run history.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, RwLock};

use omni_alerts::{AlertGate, AlertLayer, TaskFailureGate};
use omni_store::cbor::Extra;
use omni_store::entity::{EntityWrite as _, UpsertOpts};
use omni_tasks::health::SUSTAINED_FAILURE;
use omni_tasks::{TaskRunData, TaskRunStatus, Trigger};
use omni_testkit::TestApp;
use tracing_subscriber::layer::SubscriberExt as _;

const TITLE: &str = "Error running task \"ClaudeResets\"";

async fn insert(store: &omni_store::Store, task: &str, minute: i64, status: TaskRunStatus) {
    let run = TaskRunData {
        run_id: format!("{task}:{minute}"),
        task_name: task.into(),
        trigger: Trigger::Schedule,
        scheduled_for: None,
        started_at: minute * 60_000,
        finished_at: Some(minute * 60_000 + 1_000),
        error: (status == TaskRunStatus::Error).then(|| "request timed out".into()),
        status,
        summary: None,
        extra: Extra::default(),
    };
    store
        .write(move |tx| tx.upsert(&run, UpsertOpts::default()))
        .await
        .unwrap();
}

async fn emit(app: &TestApp, gate: &Arc<dyn AlertGate>, title: &'static str) {
    let (layer, worker) = AlertLayer::new(
        app.ctx.pushover.clone(),
        Arc::new(RwLock::new(vec![gate.clone()])),
        app.ctx.clock.clone(),
    );
    let subscriber = tracing_subscriber::registry().with(layer);
    tracing::subscriber::with_default(subscriber, || {
        tracing::error!(target: "Scheduler", error = "request timed out", "{title}");
    });
    worker.run().await;
}

#[tokio::test]
async fn a_minute_poller_alerts_only_after_thirty_minutes_of_failures() {
    let app = TestApp::new().await;
    let store = app.ctx.store.clone();
    let gate: Arc<dyn AlertGate> = Arc::new(TaskFailureGate::new(
        store.clone(),
        &["CodexResets", "ClaudeResets"],
        SUSTAINED_FAILURE,
    ));

    // Isolated timeouts between successes never notify.
    insert(&store, "ClaudeResets", 0, TaskRunStatus::Error).await;
    emit(&app, &gate, TITLE).await;
    insert(&store, "ClaudeResets", 1, TaskRunStatus::Success).await;
    for minute in 2..=20 {
        insert(&store, "ClaudeResets", minute, TaskRunStatus::Error).await;
    }
    emit(&app, &gate, TITLE).await;
    assert!(
        app.pushes.all().is_empty(),
        "a 19-minute streak must not notify"
    );

    // Another task's history does not count toward this one.
    for minute in 0..=40 {
        insert(&store, "CodexResets", minute, TaskRunStatus::Error).await;
    }
    emit(&app, &gate, TITLE).await;
    assert!(app.pushes.all().is_empty());

    for minute in 21..=30 {
        insert(&store, "ClaudeResets", minute, TaskRunStatus::Error).await;
    }
    emit(&app, &gate, TITLE).await;
    let pushes = app.pushes.all();
    assert_eq!(pushes.len(), 1);
    assert_eq!(
        pushes[0].message.title.as_deref(),
        Some("Error: Error running task \"ClaudeResets\"")
    );
}

#[tokio::test]
async fn other_titles_are_left_to_other_gates() {
    let app = TestApp::new().await;
    let gate = TaskFailureGate::new(app.ctx.store.clone(), &["ClaudeResets"], SUSTAINED_FAILURE);
    assert!(gate.applies("Catch-up run of \"ClaudeResets\" failed"));
    assert!(!gate.applies("Error running task \"PetTracker\""));
    assert!(!gate.applies("Rolling summary failed"));
}
