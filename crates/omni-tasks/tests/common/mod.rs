//! Shared fixtures for the omni-tasks integration tests.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use jiff::tz::TimeZone;
use omni_core::clock::SharedClock;
use omni_store::{Store, StoreOptions};
use omni_tasks::{
    CronSchedule, EventBus, RunContext, RunLogs, Task, TaskError, TaskOptions, TaskRegistry,
};
use tokio_util::task::TaskTracker;

pub fn vancouver() -> TimeZone {
    TimeZone::get("America/Vancouver").unwrap()
}

/// `new Date(2026, 6, day, hour)` in America/Vancouver.
pub fn local_time(day: i8, hour: i8) -> i64 {
    jiff::civil::date(2026, 7, day)
        .at(hour, 0, 0, 0)
        .to_zoned(vancouver())
        .unwrap()
        .timestamp()
        .as_millisecond()
}

pub struct Harness {
    pub store: Store,
    pub registry: TaskRegistry,
    pub bus: EventBus,
    pub logs: RunLogs,
    pub tracker: TaskTracker,
    _dir: tempfile::TempDir,
}

pub async fn harness(clock: SharedClock) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(
        &dir.path().join("docstore.db"),
        StoreOptions::new(clock.clone()),
    )
    .await
    .unwrap();
    let bus = EventBus::new(64);
    let logs = RunLogs::new(bus.clone(), clock.clone());
    let tracker = TaskTracker::new();
    let registry = TaskRegistry::new(
        store.clone(),
        clock,
        bus.clone(),
        tracker.clone(),
        logs.clone(),
    );
    Harness {
        store,
        registry,
        bus,
        logs,
        tracker,
        _dir: dir,
    }
}

type Behavior = Arc<dyn Fn(&RunContext) -> BoxFuture<'static, Result<(), TaskError>> + Send + Sync>;

/// A scriptable task: counts runs, records manual inputs and runs `behavior`.
pub struct FakeTask {
    pub name: String,
    pub schedule: CronSchedule,
    pub options: TaskOptions,
    pub manual_input: bool,
    pub runs: AtomicUsize,
    pub inputs: Mutex<Vec<serde_json::Value>>,
    pub contexts: Mutex<Vec<RunContext>>,
    behavior: Behavior,
}

impl FakeTask {
    pub fn new(name: &str, schedule: &str) -> Self {
        Self {
            name: name.to_owned(),
            schedule: CronSchedule::parse(schedule, &vancouver()).unwrap(),
            options: TaskOptions::default(),
            manual_input: false,
            runs: AtomicUsize::new(0),
            inputs: Mutex::new(Vec::new()),
            contexts: Mutex::new(Vec::new()),
            behavior: Arc::new(|_| Box::pin(async { Ok(()) })),
        }
    }

    pub fn on_startup(mut self) -> Self {
        self.options.run_on_startup = true;
        self
    }

    pub fn with_options(mut self, options: TaskOptions) -> Self {
        self.options = options;
        self
    }

    pub fn accepting_input(mut self) -> Self {
        self.manual_input = true;
        self
    }

    pub fn behaving(
        mut self,
        behavior: impl Fn(&RunContext) -> BoxFuture<'static, Result<(), TaskError>>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        self.behavior = Arc::new(behavior);
        self
    }

    pub fn runs(&self) -> usize {
        self.runs.load(Ordering::SeqCst)
    }

    pub fn inputs(&self) -> Vec<serde_json::Value> {
        self.inputs.lock().unwrap().clone()
    }
}

impl Task for FakeTask {
    fn name(&self) -> &str {
        &self.name
    }

    fn schedule(&self) -> &CronSchedule {
        &self.schedule
    }

    fn options(&self) -> TaskOptions {
        self.options
    }

    fn run<'a>(&'a self, cx: &'a RunContext) -> BoxFuture<'a, Result<(), TaskError>> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        self.contexts.lock().unwrap().push(cx.clone());
        (self.behavior)(cx)
    }

    fn accepts_manual_input(&self) -> bool {
        self.manual_input
    }

    fn run_manual<'a>(
        &'a self,
        cx: &'a RunContext,
        input: serde_json::Value,
    ) -> BoxFuture<'a, Result<(), TaskError>> {
        self.inputs.lock().unwrap().push(input);
        self.contexts.lock().unwrap().push(cx.clone());
        (self.behavior)(cx)
    }
}
