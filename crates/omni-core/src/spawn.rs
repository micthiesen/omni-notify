//! The only sanctioned ways to run background work (no fire-and-forget).

use std::future::Future;

use tokio::task::JoinHandle;
use tokio_util::task::TaskTracker;
use tracing::Instrument as _;

/// Spawns `fut` on `tracker`, instrumented with the caller's current span so
/// run attribution and log capture follow the work.
pub fn spawn_tracked<F>(tracker: &TaskTracker, name: &'static str, fut: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    tracing::trace!(target: "spawn", task = name, "spawning tracked task");
    tracker.spawn(fut.instrument(tracing::Span::current()))
}

/// Runs `fut` to completion on `tracker` even if the caller is dropped.
///
/// A panic inside `fut` is resumed in the caller. Tracked tasks are never
/// aborted, so the only other join failure is runtime shutdown, which is
/// reported as a panic because no value can be produced.
pub async fn must_complete<F, T>(tracker: &TaskTracker, fut: F) -> T
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    match spawn_tracked(tracker, "must_complete", fut).await {
        Ok(value) => value,
        Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
        Err(error) => std::panic::resume_unwind(Box::new(format!(
            "must_complete task did not finish: {error}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    #[tokio::test(start_paused = true)]
    async fn must_complete_survives_dropped_caller() {
        let tracker = TaskTracker::new();
        let done = Arc::new(AtomicBool::new(false));
        let flag = done.clone();
        let caller = must_complete(&tracker, async move {
            tokio::time::sleep(Duration::from_secs(5)).await;
            flag.store(true, Ordering::SeqCst);
        });
        let _ = tokio::time::timeout(Duration::from_secs(1), caller).await;
        tracker.close();
        tracker.wait().await;
        assert!(done.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn spawn_tracked_returns_output() {
        let tracker = TaskTracker::new();
        let handle = spawn_tracked(&tracker, "test", async { 7 });
        assert_eq!(handle.await.ok(), Some(7));
    }
}
