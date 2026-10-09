//! One loop per task (mitools `Scheduler.ts` on Effect `Schedule.cron`).
//!
//! Each loop sleeps until the first cron match after the previous run
//! completed, so a fire that lands during a run is skipped. A uniform jitter
//! in `[0, jitter)` precedes every run (startup runs included). A started run
//! is never interrupted: shutdown stops the sleeping and waits for runs in
//! flight.

use std::sync::Arc;
use std::time::Duration;

use jiff::Timestamp;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::TaskRegistry;
use crate::registry::{Entry, ExecuteError};

const LOG: &str = "Scheduler";

/// Starts the per-task loops.
pub struct Scheduler;

impl Scheduler {
    /// Starts one loop per registered task on `tracker`. The returned task
    /// resolves after `shutdown` fires and every loop (with its run in
    /// flight) has finished. Shutdown also stops the registry from starting
    /// queued manual runs.
    pub fn start(
        registry: TaskRegistry,
        shutdown: CancellationToken,
        tracker: &TaskTracker,
    ) -> JoinHandle<()> {
        let loop_tracker = tracker.clone();
        omni_core::spawn::spawn_tracked(tracker, "scheduler", async move {
            let entries = registry.entries();
            let mut loops = Vec::with_capacity(entries.len());
            for entry in entries {
                tracing::info!(
                    target: LOG,
                    "Registered task \"{}\" with schedule \"{}\"",
                    entry.name(),
                    entry.task.schedule().as_str()
                );
                loops.push(omni_core::spawn::spawn_tracked(
                    &loop_tracker,
                    "scheduler_loop",
                    task_loop(registry.clone(), entry, shutdown.clone()),
                ));
            }
            tracing::info!(target: LOG, "Started {} scheduled task(s)", loops.len());
            shutdown.cancelled().await;
            if !loops.is_empty() {
                tracing::info!(target: LOG, "Stopping {} scheduled task(s)...", loops.len());
            }
            registry.shutdown();
            for handle in loops {
                if let Err(error) = handle.await {
                    tracing::error!(target: LOG, error = %error, "Scheduler loop panicked");
                }
            }
        })
    }
}

async fn task_loop(registry: TaskRegistry, entry: Arc<Entry>, shutdown: CancellationToken) {
    let name = entry.name().to_owned();
    if entry.task.options().run_on_startup {
        run_once(&registry, &entry, &shutdown).await;
    }
    let mut last_fire: Option<Timestamp> = None;
    while !shutdown.is_cancelled() {
        let now = registry.clock().now();
        // Never fire the same occurrence twice when the wall clock reads
        // slightly behind the timer that woke us.
        let from = last_fire.map_or(now, |fired| fired.max(now));
        let Some(next) = entry.task.schedule().next_after(from) else {
            tracing::error!(
                target: LOG,
                "Scheduler loop for \"{name}\" stopped: the schedule has no next occurrence"
            );
            return;
        };
        let delay = duration_until(now, next);
        tokio::select! {
            () = shutdown.cancelled() => return,
            () = tokio::time::sleep(delay) => {}
        }
        last_fire = Some(next);
        run_once(&registry, &entry, &shutdown).await;
    }
}

fn duration_until(now: Timestamp, next: Timestamp) -> Duration {
    let ms = next.as_millisecond().saturating_sub(now.as_millisecond());
    Duration::from_millis(u64::try_from(ms).unwrap_or(0))
}

async fn run_once(registry: &TaskRegistry, entry: &Arc<Entry>, shutdown: &CancellationToken) {
    let jitter = entry.task.options().jitter;
    let jitter_ms = u64::try_from(jitter.as_millis()).unwrap_or(u64::MAX);
    if jitter_ms > 0 {
        let delay = Duration::from_millis(rand::random_range(0..jitter_ms));
        tokio::select! {
            () = shutdown.cancelled() => return,
            () = tokio::time::sleep(delay) => {}
        }
    }
    let name = entry.name().to_owned();
    tracing::debug!(target: LOG, "Running task: {name}");
    match registry.run_scheduled(entry.clone()).await {
        Ok(_) | Err(ExecuteError::Shutdown) => {}
        Err(error) => {
            tracing::error!(target: LOG, error = %error, "Error running task \"{name}\"");
        }
    }
}
