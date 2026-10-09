//! The task registry: durable run history,
//! per-task serialization of every trigger, manual runs and catch-up.

use std::collections::{HashMap, HashSet};
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use futures::FutureExt as _;
use omni_core::clock::SharedClock;
use omni_store::{LogLine, Store, StoreError};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tracing::Instrument as _;

use crate::catch_up::{self, CatchUpDecision};
use crate::log_capture::run_span;
use crate::persistence::{self, RunEnd, RunLogsData, RunStart, TaskRunData, TaskRunStatus};
use crate::{
    DuplicateTaskError, EventBus, RunContext, RunLogEvent, RunLogs, RunNowError, RunOutcome, Task,
    TaskInfo, TaskRunEvent, TaskRunEventKind, Trigger,
};

/// The TS registry logs through `Logger.named("Main").extend("TaskRegistry")`.
pub(crate) const LOG: &str = "Main:TaskRegistry";
/// `cronTask.getNextRuns(3)`.
const NEXT_RUNS: usize = 3;

pub(crate) struct Entry {
    pub(crate) task: Arc<dyn Task>,
    /// One permit: scheduled, manual, startup and catch-up runs never overlap.
    permit: Semaphore,
}

impl Entry {
    pub(crate) fn name(&self) -> &str {
        self.task.name()
    }
}

#[derive(Default)]
struct RunState {
    running: HashSet<String>,
    /// Runs reserved (waiting for or holding the permit) per task.
    queued: HashMap<String, usize>,
    /// Tasks that have recorded at least one run in this process.
    has_run: HashSet<String>,
}

struct RegistryInner {
    store: Store,
    clock: SharedClock,
    bus: EventBus,
    tracker: TaskTracker,
    logs: RunLogs,
    entries: Mutex<Vec<Arc<Entry>>>,
    state: Mutex<RunState>,
    /// Cancelled when the scheduler stops; queued runs that have not started
    /// are abandoned, started runs always finish.
    shutdown: CancellationToken,
    initialized: AtomicBool,
}

/// Why an execution did not produce a successful run.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ExecuteError {
    /// The task failed (or panicked); the run is recorded as `error`.
    #[error("{message}")]
    Task { run_id: String, message: String },
    /// The run could not be established (nothing ran).
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("shutting down")]
    Shutdown,
}

/// Releases a queue reservation when the execution ends.
struct QueueReservation {
    registry: TaskRegistry,
    name: String,
}

impl Drop for QueueReservation {
    fn drop(&mut self) {
        let mut state = self.registry.state();
        match state.queued.get_mut(&self.name) {
            Some(count) if *count > 1 => *count -= 1,
            _ => {
                state.queued.remove(&self.name);
            }
        }
    }
}

/// Registry of every scheduled task; cheap to clone.
#[derive(Clone)]
pub struct TaskRegistry {
    inner: Arc<RegistryInner>,
}

impl TaskRegistry {
    pub fn new(
        store: Store,
        clock: SharedClock,
        bus: EventBus,
        tracker: TaskTracker,
        logs: RunLogs,
    ) -> Self {
        Self {
            inner: Arc::new(RegistryInner {
                store,
                clock,
                bus,
                tracker,
                logs,
                entries: Mutex::new(Vec::new()),
                state: Mutex::new(RunState::default()),
                shutdown: CancellationToken::new(),
                initialized: AtomicBool::new(false),
            }),
        }
    }

    fn entries_lock(&self) -> MutexGuard<'_, Vec<Arc<Entry>>> {
        self.inner
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn state(&self) -> MutexGuard<'_, RunState> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Registered entries in registration order.
    pub(crate) fn entries(&self) -> Vec<Arc<Entry>> {
        self.entries_lock().clone()
    }

    fn entry(&self, name: &str) -> Option<Arc<Entry>> {
        self.entries_lock()
            .iter()
            .find(|entry| entry.name() == name)
            .cloned()
    }

    pub(crate) fn clock(&self) -> &SharedClock {
        &self.inner.clock
    }

    /// The live per-run log buffers this registry captures into; work outside a
    /// task run (email processing) captures through the same buffers.
    pub fn run_log_buffers(&self) -> RunLogs {
        self.inner.logs.clone()
    }

    /// Registers a task; names are unique.
    pub fn track(&self, task: Arc<dyn Task>) -> Result<(), DuplicateTaskError> {
        let mut entries = self.entries_lock();
        let name = task.name().to_owned();
        if entries.iter().any(|entry| entry.name() == name) {
            return Err(DuplicateTaskError(name));
        }
        entries.push(Arc::new(Entry {
            task,
            permit: Semaphore::new(1),
        }));
        Ok(())
    }

    /// Registered task names, sorted.
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .entries_lock()
            .iter()
            .map(|entry| entry.name().to_owned())
            .collect();
        names.sort();
        names
    }

    /// Stops admitting queued runs: runs waiting for their task's permit are
    /// abandoned; runs already started finish. Called by the scheduler on
    /// shutdown (TS `shutdownEffect`); idempotent.
    pub fn shutdown(&self) {
        self.inner.shutdown.cancel();
    }

    /// `markInterruptedRuns`, once: runs left `running` by a previous process
    /// become errors.
    pub async fn initialize(&self) -> Result<(), StoreError> {
        if self.inner.initialized.load(Ordering::SeqCst) {
            return Ok(());
        }
        let now = self.inner.clock.now_ms();
        let interrupted = persistence::mark_interrupted_runs(&self.inner.store, now).await?;
        self.inner.initialized.store(true, Ordering::SeqCst);
        if interrupted > 0 {
            tracing::warn!(
                target: LOG,
                "Marked {interrupted} interrupted task run(s) as errors"
            );
        }
        Ok(())
    }

    /// Queues a manual run and returns its run id at once. Rejects when the
    /// task is already running or queued; the check and the reservation are
    /// atomic, so one of two simultaneous requests always fails.
    pub fn run_now(
        &self,
        name: &str,
        input: Option<serde_json::Value>,
    ) -> Result<String, RunNowError> {
        let entry = self.entry(name).ok_or_else(|| RunNowError::NotFound {
            name: name.to_owned(),
        })?;
        if input.is_some() && !entry.task.accepts_manual_input() {
            return Err(RunNowError::ManualInputUnsupported {
                name: name.to_owned(),
            });
        }
        let reservation = {
            let mut state = self.state();
            if state.running.contains(name) || state.queued.get(name).copied().unwrap_or(0) > 0 {
                return Err(RunNowError::AlreadyRunning {
                    name: name.to_owned(),
                });
            }
            *state.queued.entry(name.to_owned()).or_insert(0) += 1;
            QueueReservation {
                registry: self.clone(),
                name: name.to_owned(),
            }
        };
        let run_id = persistence::make_run_id(name);
        let registry = self.clone();
        let task_name = name.to_owned();
        let id = run_id.clone();
        self.spawn_detached(async move {
            let _reservation = reservation;
            let result = registry
                .execute(entry, Trigger::Manual, Some(id), None, input)
                .await;
            match result {
                Ok(_) | Err(ExecuteError::Shutdown) => {}
                Err(error) => {
                    tracing::error!(
                        target: LOG,
                        error = %error,
                        "Manual run of \"{task_name}\" failed"
                    );
                }
            }
        });
        Ok(run_id)
    }

    /// Queues a manual run (waiting behind an active run) and resolves once
    /// that exact run is durably finished. The run is never cancelled by
    /// dropping this future: it completes on the app tracker.
    pub async fn run_now_and_wait(
        &self,
        name: &str,
        input: Option<serde_json::Value>,
    ) -> Result<RunOutcome, RunNowError> {
        let entry = self.entry(name).ok_or_else(|| RunNowError::NotFound {
            name: name.to_owned(),
        })?;
        if input.is_some() && !entry.task.accepts_manual_input() {
            return Err(RunNowError::ManualInputUnsupported {
                name: name.to_owned(),
            });
        }
        let reservation = self.reserve(name);
        let run_id = persistence::make_run_id(name);
        let registry = self.clone();
        let handle = self.spawn_detached(async move {
            let _reservation = reservation;
            registry
                .execute(entry, Trigger::Manual, Some(run_id), None, input)
                .await
        });
        match handle.await {
            Ok(Ok(run)) => Ok(RunOutcome { run }),
            Ok(Err(ExecuteError::Task { run_id, message })) => {
                Err(RunNowError::RunFailed { run_id, message })
            }
            Ok(Err(ExecuteError::Store(error))) => Err(RunNowError::Store(error)),
            Ok(Err(ExecuteError::Shutdown)) | Err(_) => Err(RunNowError::Shutdown),
        }
    }

    fn reserve(&self, name: &str) -> QueueReservation {
        *self.state().queued.entry(name.to_owned()).or_insert(0) += 1;
        QueueReservation {
            registry: self.clone(),
            name: name.to_owned(),
        }
    }

    /// A scheduled fire (the scheduler's `run` wrapper): reserves a queue
    /// slot, then executes on the task's permit. Runs to completion even if
    /// the caller is dropped.
    pub(crate) async fn run_scheduled(
        &self,
        entry: Arc<Entry>,
    ) -> Result<TaskRunData, ExecuteError> {
        let reservation = self.reserve(entry.name());
        let registry = self.clone();
        let handle = self.spawn_detached(async move {
            let _reservation = reservation;
            registry
                .execute(entry, Trigger::Schedule, None, None, None)
                .await
        });
        handle.await.unwrap_or(Err(ExecuteError::Shutdown))
    }

    /// Spawns on the app tracker under a root span, so the work is never
    /// attributed to whatever run or request scheduled it.
    fn spawn_detached<F>(&self, fut: F) -> tokio::task::JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let root = tracing::debug_span!(target: LOG, parent: None, "task_execute");
        root.in_scope(|| omni_core::spawn::spawn_tracked(&self.inner.tracker, "task_run", fut))
    }

    /// Waits for the task's permit, records the run start and cursor, runs
    /// the task inside its run span, then records the outcome and persists
    /// the captured logs.
    async fn execute(
        &self,
        entry: Arc<Entry>,
        trigger: Trigger,
        run_id: Option<String>,
        scheduled_for: Option<i64>,
        input: Option<serde_json::Value>,
    ) -> Result<TaskRunData, ExecuteError> {
        let inner = &self.inner;
        let _permit = tokio::select! {
            biased;
            () = inner.shutdown.cancelled() => return Err(ExecuteError::Shutdown),
            permit = entry.permit.acquire() => permit.map_err(|_| ExecuteError::Shutdown)?,
        };
        let task = entry.task.clone();
        let name = task.name().to_owned();
        let options = task.options();
        // The scheduler fires runOnStartup tasks through the scheduled path;
        // startup state is consumed only once the run row exists.
        let actual_trigger = if trigger == Trigger::Schedule
            && options.run_on_startup
            && !self.state().has_run.contains(&name)
        {
            Trigger::Startup
        } else {
            trigger
        };
        let started_at = inner.clock.now_ms();
        let run = persistence::record_run_start_and_mark_schedule(
            &inner.store,
            RunStart {
                task_name: name.clone(),
                trigger: actual_trigger,
                schedule: task.schedule().as_str().to_owned(),
                evaluated_through: scheduled_for.unwrap_or(started_at),
                run_id: run_id.unwrap_or_else(|| persistence::make_run_id(&name)),
                scheduled_for,
                started_at,
            },
        )
        .await?;
        {
            let mut state = self.state();
            state.has_run.insert(name.clone());
            state.running.insert(name.clone());
        }
        inner.logs.start(&run.run_id, &name);
        inner.bus.emit_task_run(TaskRunEvent {
            kind: TaskRunEventKind::RunStarted,
            task_name: name.clone(),
        });

        let cx = RunContext {
            run_id: run.run_id.clone(),
            task_name: name.clone(),
            trigger: actual_trigger,
            scheduled_for,
            cancel: inner.shutdown.child_token(),
        };
        let outcome = {
            let manual_input = input.filter(|_| actual_trigger == Trigger::Manual);
            let work = async {
                match manual_input {
                    Some(value) => task.run_manual(&cx, value).await,
                    None => task.run(&cx).await,
                }
            };
            AssertUnwindSafe(work)
                .catch_unwind()
                .instrument(run_span(&run.run_id, &name))
                .await
        };
        let failure = match outcome {
            Ok(Ok(())) => None,
            Ok(Err(error)) => Some(error.message),
            Err(panic) => Some(format!("panic: {}", panic_message(panic.as_ref()))),
        };

        let end = RunEnd {
            status: if failure.is_some() {
                TaskRunStatus::Error
            } else {
                TaskRunStatus::Success
            },
            error: failure.clone(),
            summary: task.last_run_summary(),
            finished_at: inner.clock.now_ms(),
        };
        let settled =
            match persistence::record_run_end(&inner.store, &run.run_id, end.clone()).await {
                Ok(Some(settled)) => settled,
                Ok(None) => settle_locally(run.clone(), end),
                Err(error) => {
                    tracing::error!(target: LOG, error = %error, "Record task run end failed");
                    settle_locally(run.clone(), end)
                }
            };
        self.finish_run(&run.run_id, &name).await;

        match failure {
            None => Ok(settled),
            Some(message) => Err(ExecuteError::Task {
                run_id: run.run_id,
                message,
            }),
        }
    }

    /// Persists the captured lines, tells log viewers the run ended, and
    /// clears the running flag. Must follow the recorded run end, so `end`
    /// subscribers read a settled run.
    async fn finish_run(&self, run_id: &str, name: &str) {
        let inner = &self.inner;
        if let Some((task_name, lines, dropped)) = inner.logs.take(run_id) {
            let saved = persistence::save_run_logs(
                &inner.store,
                RunLogsData {
                    run_id: run_id.to_owned(),
                    task_name,
                    lines,
                    dropped,
                },
            )
            .await;
            if let Err(error) = saved {
                tracing::error!(target: LOG, error = %error, "Finish task log capture failed");
            }
        }
        inner.bus.emit_run_log(RunLogEvent::End {
            run_id: run_id.to_owned(),
        });
        self.state().running.remove(name);
        inner.bus.emit_task_run(TaskRunEvent {
            kind: TaskRunEventKind::RunFinished,
            task_name: name.to_owned(),
        });
    }

    /// Every task with its last run and next three fire times.
    pub async fn list(&self) -> Result<Vec<TaskInfo>, StoreError> {
        let now = self.inner.clock.now();
        let mut out = Vec::new();
        for entry in self.entries() {
            let name = entry.name().to_owned();
            let last_run = persistence::get_last_run(&self.inner.store, &name).await?;
            let schedule = entry.task.schedule();
            out.push(TaskInfo {
                display_name: entry.task.display_name().map(str::to_owned),
                schedule: schedule.as_str().to_owned(),
                running: self.state().running.contains(&name),
                next_runs: schedule
                    .next_n(now, NEXT_RUNS)
                    .into_iter()
                    .map(|t| omni_core::js::to_iso_string(t.as_millisecond()))
                    .collect(),
                last_run: last_run.as_ref().map(omni_api::runs::Run::from),
                name,
            });
        }
        Ok(out)
    }

    /// Whether a run of `name` is in flight.
    pub fn is_running(&self, name: &str) -> bool {
        self.state().running.contains(name)
    }

    /// Newest runs first, optionally for one task.
    pub async fn recent_runs(
        &self,
        task: Option<&str>,
        limit: usize,
    ) -> Result<Vec<TaskRunData>, StoreError> {
        persistence::get_runs(&self.inner.store, task, limit).await
    }

    /// One run with its log lines: the live buffer while the run is in
    /// flight, else the persisted row, else none.
    pub async fn run_logs(
        &self,
        run_id: &str,
    ) -> Result<Option<(TaskRunData, Vec<LogLine>, u64)>, StoreError> {
        let Some(run) = persistence::get_run(&self.inner.store, run_id).await? else {
            return Ok(None);
        };
        if let Some((lines, dropped)) = self.inner.logs.active(run_id) {
            return Ok(Some((run, lines, dropped)));
        }
        let stored = persistence::get_run_logs(&self.inner.store, run_id).await?;
        Ok(Some(match stored {
            Some(logs) => (run, logs.lines, logs.dropped),
            None => (run, Vec::new(), 0),
        }))
    }

    /// Catch-up: for each infrequent task, recovers at most the newest missed
    /// occurrence, sequentially, and advances every task's cursor.
    pub async fn recover_missed(&self) -> Result<(), StoreError> {
        let store = &self.inner.store;
        let current_time = self.inner.clock.now_ms();
        let now = omni_core::clock::timestamp_from_ms(current_time);
        let mut recoveries = Vec::new();

        for entry in self.entries() {
            let name = entry.name().to_owned();
            let schedule = entry.task.schedule();
            let expr = schedule.as_str();
            let state = persistence::get_task_schedule_state(store, &name).await?;
            if state.as_ref().is_some_and(|s| s.schedule != expr) {
                tracing::info!(
                    target: LOG,
                    "Schedule changed for \"{name}\"; starting a new recovery baseline"
                );
                persistence::mark_schedule_evaluated(store, &name, expr, current_time).await?;
                continue;
            }
            let evaluated_through = match state {
                Some(state) => Some(state.evaluated_through),
                None => persistence::get_last_run(store, &name)
                    .await?
                    .map(|run| run.started_at),
            };
            if evaluated_through.is_none() || entry.task.options().run_on_startup {
                persistence::mark_schedule_evaluated(store, &name, expr, current_time).await?;
                continue;
            }
            match catch_up::decide(schedule, now, evaluated_through) {
                CatchUpDecision::Run { scheduled_for, .. } => {
                    recoveries.push((entry.clone(), scheduled_for));
                }
                CatchUpDecision::Stale { scheduled_for, .. } => {
                    tracing::info!(
                        target: LOG,
                        "Skipping stale missed run of \"{name}\" from {}",
                        omni_core::js::to_iso_string(scheduled_for)
                    );
                    persistence::mark_schedule_evaluated(store, &name, expr, current_time).await?;
                }
                CatchUpDecision::Disabled { .. } | CatchUpDecision::None => {
                    persistence::mark_schedule_evaluated(store, &name, expr, current_time).await?;
                }
            }
        }

        // Sequential, so a reboot cannot unleash several expensive tasks at once.
        for (entry, scheduled_for) in recoveries {
            let name = entry.name().to_owned();
            tracing::info!(
                target: LOG,
                "Recovering missed run of \"{name}\" from {}",
                omni_core::js::to_iso_string(scheduled_for)
            );
            let registry = self.clone();
            let handle = self.spawn_detached(async move {
                registry
                    .execute(entry, Trigger::Catchup, None, Some(scheduled_for), None)
                    .await
            });
            match handle.await {
                Ok(Ok(_)) | Ok(Err(ExecuteError::Shutdown)) => {}
                Ok(Err(error)) => {
                    tracing::error!(
                        target: LOG,
                        error = %error,
                        "Catch-up run of \"{name}\" failed"
                    );
                }
                Err(error) => {
                    tracing::error!(
                        target: LOG,
                        error = %error,
                        "Catch-up run of \"{name}\" failed"
                    );
                }
            }
        }
        Ok(())
    }
}

fn settle_locally(mut run: TaskRunData, end: RunEnd) -> TaskRunData {
    run.status = end.status;
    run.error = end.error;
    run.summary = end.summary;
    run.finished_at = Some(end.finished_at);
    run
}

fn panic_message(panic: &(dyn std::any::Any + Send)) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "task panicked".to_owned())
}
