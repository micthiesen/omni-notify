//! Shared fakes for the integration tests.
#![allow(dead_code, clippy::unwrap_used)]

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use futures::future::BoxFuture;
use omni_personal::reset_alerts::{NotifyError, ResetAlert, ResetNotifier};
use tokio::sync::{Notify, oneshot};

/// A scripted push channel: replies in order (default `Ok`), optionally
/// blocking the next call until released.
#[derive(Default)]
pub struct FakeNotifier {
    pub disabled: bool,
    calls: AtomicUsize,
    replies: Mutex<VecDeque<Result<(), NotifyError>>>,
    gate: Mutex<Option<oneshot::Receiver<()>>>,
    pub called: Notify,
    pub sent: Mutex<Vec<ResetAlert>>,
}

impl FakeNotifier {
    pub fn disabled() -> Self {
        Self {
            disabled: true,
            ..Self::default()
        }
    }

    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    pub fn reply_once(&self, reply: Result<(), NotifyError>) {
        self.replies.lock().unwrap().push_back(reply);
    }

    /// Blocks the next call until the returned sender fires (or is dropped).
    pub fn block_next(&self) -> oneshot::Sender<()> {
        let (tx, rx) = oneshot::channel();
        *self.gate.lock().unwrap() = Some(rx);
        tx
    }
}

impl ResetNotifier for FakeNotifier {
    fn enabled(&self) -> bool {
        !self.disabled
    }

    fn notify<'a>(&'a self, alert: &'a ResetAlert) -> BoxFuture<'a, Result<(), NotifyError>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.sent.lock().unwrap().push(alert.clone());
            let gate = self.gate.lock().unwrap().take();
            self.called.notify_waiters();
            if let Some(gate) = gate {
                let _ = gate.await;
            }
            self.replies.lock().unwrap().pop_front().unwrap_or(Ok(()))
        })
    }
}

/// Waits until `notifier` has been called `n` times.
pub async fn wait_for_calls(notifier: &FakeNotifier, n: usize) {
    for _ in 0..500 {
        if notifier.calls() >= n {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    panic!("notifier was not called {n} times");
}
