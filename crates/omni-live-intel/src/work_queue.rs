//! Bounded work queues with graceful close.
//!
//! `run` awaits a job under a permit; `fork` detaches it onto the app's task
//! tracker. `close` stops admission, waits up to 30 s for admitted work, then
//! interrupts detached jobs (dropping their futures, so drop guards run) and
//! waits for the rest. `queued` counts admitted jobs waiting for a permit and
//! `running` those holding one.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use tokio::sync::{Notify, Semaphore};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

pub const DRAIN_WINDOW: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("work queue is closed")]
pub struct WorkQueueClosed;

struct Inner {
    name: &'static str,
    semaphore: Semaphore,
    accepting: AtomicBool,
    outstanding: AtomicUsize,
    queued: AtomicUsize,
    running: AtomicUsize,
    idle: Notify,
    interrupt: CancellationToken,
    tracker: TaskTracker,
}

#[derive(Clone)]
pub struct WorkQueue {
    inner: Arc<Inner>,
}

/// Tracks one admitted job from admission to completion or drop.
struct Admission {
    inner: Arc<Inner>,
    started: bool,
}

impl Admission {
    fn start(&mut self) {
        self.started = true;
        self.inner.queued.fetch_sub(1, Ordering::SeqCst);
        self.inner.running.fetch_add(1, Ordering::SeqCst);
    }
}

impl Drop for Admission {
    fn drop(&mut self) {
        if self.started {
            self.inner.running.fetch_sub(1, Ordering::SeqCst);
        } else {
            self.inner.queued.fetch_sub(1, Ordering::SeqCst);
        }
        if self.inner.outstanding.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.inner.idle.notify_waiters();
        }
    }
}

impl WorkQueue {
    /// A queue running at most `concurrency` jobs; detached jobs are spawned on `tracker`.
    pub fn new(name: &'static str, concurrency: usize, tracker: TaskTracker) -> Self {
        Self {
            inner: Arc::new(Inner {
                name,
                semaphore: Semaphore::new(concurrency),
                accepting: AtomicBool::new(true),
                outstanding: AtomicUsize::new(0),
                queued: AtomicUsize::new(0),
                running: AtomicUsize::new(0),
                idle: Notify::new(),
                interrupt: CancellationToken::new(),
                tracker,
            }),
        }
    }

    /// Jobs holding a permit.
    pub fn running(&self) -> usize {
        self.inner.running.load(Ordering::SeqCst)
    }

    /// Admitted jobs waiting for a permit.
    pub fn queued(&self) -> usize {
        self.inner.queued.load(Ordering::SeqCst)
    }

    fn admit(&self) -> Result<Admission, WorkQueueClosed> {
        if !self.inner.accepting.load(Ordering::SeqCst) {
            return Err(WorkQueueClosed);
        }
        self.inner.outstanding.fetch_add(1, Ordering::SeqCst);
        self.inner.queued.fetch_add(1, Ordering::SeqCst);
        Ok(Admission {
            inner: Arc::clone(&self.inner),
            started: false,
        })
    }

    async fn execute<F: Future>(inner: &Inner, mut admission: Admission, job: F) -> F::Output {
        // The semaphore is never closed, so acquire only fails if it were.
        let _permit = inner.semaphore.acquire().await;
        admission.start();
        let output = job.await;
        drop(admission);
        output
    }

    /// Runs `job` under a permit and returns its output; dropping the returned
    /// future drops the job and releases its admission.
    pub async fn run<F: Future>(&self, job: F) -> Result<F::Output, WorkQueueClosed> {
        let admission = self.admit()?;
        Ok(Self::execute(&self.inner, admission, job).await)
    }

    /// Detaches `job`; silently dropped when the queue is closed. Admission is
    /// synchronous, so it cannot leak if the caller is cancelled.
    pub fn fork<F>(&self, job: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let Ok(admission) = self.admit() else {
            return;
        };
        let inner = Arc::clone(&self.inner);
        omni_core::spawn::spawn_tracked(&self.inner.tracker, self.inner.name, async move {
            let interrupt = inner.interrupt.clone();
            tokio::select! {
                () = Self::execute(&inner, admission, job) => {}
                () = interrupt.cancelled() => {
                    tracing::warn!(target: crate::LOG, queue = inner.name, "Interrupted a job that did not finish within the drain window");
                }
            }
        });
    }

    async fn wait_idle(&self) {
        loop {
            let notified = self.inner.idle.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.inner.outstanding.load(Ordering::SeqCst) == 0 {
                return;
            }
            notified.await;
        }
    }

    /// Stops admission, drains for up to 30 s, then interrupts detached jobs.
    pub async fn close(&self) {
        self.inner.accepting.store(false, Ordering::SeqCst);
        if tokio::time::timeout(DRAIN_WINDOW, self.wait_idle())
            .await
            .is_err()
        {
            self.inner.interrupt.cancel();
            self.wait_idle().await;
        }
    }
}
