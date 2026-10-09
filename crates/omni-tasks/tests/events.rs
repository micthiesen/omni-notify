//! The run event bus. A subscriber can only stop receiving (drop) or fall
//! behind (lag); neither may affect emitters or other subscribers.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_tasks::{EventBus, TaskRunEvent, TaskRunEventKind};

fn started(name: &str) -> TaskRunEvent {
    TaskRunEvent {
        kind: TaskRunEventKind::RunStarted,
        task_name: name.to_owned(),
    }
}

#[tokio::test]
async fn isolates_a_failing_task_run_subscriber() {
    let bus = EventBus::new(4);
    let broken = bus.task_runs();
    drop(broken);
    let mut lagging = bus.task_runs();
    let mut healthy = bus.task_runs();
    for _ in 0..10 {
        bus.emit_task_run(started("Filler"));
        assert_eq!(healthy.recv().await.ok(), Some(started("Filler")));
    }
    bus.emit_task_run(started("HealthyTask"));
    assert_eq!(healthy.recv().await.ok(), Some(started("HealthyTask")));
    assert!(matches!(
        lagging.recv().await,
        Err(tokio::sync::broadcast::error::RecvError::Lagged(_))
    ));
}

#[test]
fn emitting_without_subscribers_is_not_an_error() {
    let bus = EventBus::default();
    bus.emit_task_run(started("Nobody"));
}
