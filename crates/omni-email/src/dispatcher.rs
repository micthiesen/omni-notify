//! Transport-agnostic fan-out (`src/email/dispatcher.ts`). On every mail event
//! the dispatcher polls the source for new emails and hands them to every
//! handler. Dispatch is no-drop: a bounded(1) trigger channel coalesces bursts
//! while preserving one final pass for events that land mid-pass; the source
//! cursor commits only after every handler accepted the batch, and the
//! dispatch watermark only after the commit.

use std::sync::Arc;

use futures::future::join_all;
use omni_core::email::{EmailHandler, FetchedEmail, HandlerError};
use omni_core::mail_source::{MailSource, PollError};
use omni_store::Store;
use tokio::sync::{Mutex, broadcast, mpsc};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::dispatch_state;

const LOG: &str = "Main:Email";

/// A failed pass (`EmailHandlerError`).
#[derive(Debug, thiserror::Error)]
pub enum DispatchError {
    #[error("Handler \"{handler}\" failed: {source}")]
    Handler {
        handler: String,
        #[source]
        source: HandlerError,
    },
    #[error("Handler \"{source_name}\" failed: {source}")]
    Source {
        source_name: String,
        #[source]
        source: PollError,
    },
    #[error("Handler \"{source_name} cursor\" failed: {source}")]
    Commit {
        source_name: String,
        #[source]
        source: PollError,
    },
    #[error("Handler \"{source_name}\" failed: dispatch watermark: {source}")]
    Watermark {
        source_name: String,
        #[source]
        source: omni_store::StoreError,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Signal {
    Trigger,
    Stop,
}

struct Running {
    trigger: mpsc::Sender<Signal>,
    supervisor: JoinHandle<()>,
    forwarder_cancel: CancellationToken,
    forwarder: JoinHandle<()>,
}

struct Inner {
    source: Arc<dyn MailSource>,
    source_name: String,
    handlers: Vec<Arc<dyn EmailHandler>>,
    store: Store,
}

/// The email dispatcher; cheap to clone.
#[derive(Clone)]
pub struct EmailDispatcher {
    inner: Arc<Inner>,
    tracker: TaskTracker,
    /// Lifecycle lock (start/stop are serialized) plus the running state.
    running: Arc<Mutex<Option<Running>>>,
    /// The live trigger sender, readable without the lifecycle lock so a
    /// notification during start or stop is never dropped.
    trigger: Arc<std::sync::Mutex<Option<mpsc::Sender<Signal>>>>,
}

impl EmailDispatcher {
    /// `source_name` labels errors (`"IMAP"`); handlers run in the given order.
    pub fn new(
        store: Store,
        source: Arc<dyn MailSource>,
        source_name: impl Into<String>,
        handlers: Vec<Arc<dyn EmailHandler>>,
        tracker: TaskTracker,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                source,
                source_name: source_name.into(),
                handlers,
                store,
            }),
            tracker,
            running: Arc::new(Mutex::new(None)),
            trigger: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    pub fn handler_count(&self) -> usize {
        self.inner.handlers.len()
    }

    /// One pass with errors logged (`onMailEventEffect`): the deterministic
    /// trigger for tests and manual polling.
    pub async fn poll_once(&self) {
        if let Err(error) = self.inner.process_pass().await {
            tracing::error!(target: LOG, error = %error, "Dispatcher error");
        }
    }

    /// Starts the trigger supervisor, then the source (push monitoring). If the
    /// source fails to start, the source is stopped and the supervisor joined.
    pub async fn start(&self) -> Result<(), DispatchError> {
        let mut running = self.running.lock().await;
        if running.is_some() {
            return Ok(());
        }
        let (trigger, receiver) = mpsc::channel::<Signal>(1);
        let supervisor = omni_core::spawn::spawn_tracked(
            &self.tracker,
            "email-dispatcher",
            supervise(self.inner.clone(), receiver),
        );
        let forwarder_cancel = CancellationToken::new();
        let forwarder = omni_core::spawn::spawn_tracked(
            &self.tracker,
            "email-dispatcher-events",
            forward_events(
                self.inner.source.mail_events(),
                trigger.clone(),
                forwarder_cancel.clone(),
            ),
        );
        self.set_trigger(Some(trigger.clone()));
        let state = Running {
            trigger,
            supervisor,
            forwarder_cancel,
            forwarder,
        };
        if let Err(source) = self.inner.source.start().await {
            self.inner.source.stop().await;
            self.set_trigger(None);
            stop_running(state).await;
            return Err(DispatchError::Source {
                source_name: self.inner.source_name.clone(),
                source,
            });
        }
        *running = Some(state);
        Ok(())
    }

    /// Stops the source, drains a queued final pass, then joins the supervisor.
    pub async fn stop(&self) {
        let mut running = self.running.lock().await;
        self.inner.source.stop().await;
        self.set_trigger(None);
        if let Some(state) = running.take() {
            stop_running(state).await;
        }
    }

    /// Requests a pass (`onMailEvent`): coalesced, never blocks, ignored when stopped.
    /// A full channel already holds a pending pass, which covers this event.
    pub fn notify(&self) {
        let trigger = self.trigger.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(trigger) = trigger.as_ref() {
            let _ = trigger.try_send(Signal::Trigger);
        }
    }

    fn set_trigger(&self, trigger: Option<mpsc::Sender<Signal>>) {
        *self.trigger.lock().unwrap_or_else(|p| p.into_inner()) = trigger;
    }
}

async fn stop_running(state: Running) {
    state.forwarder_cancel.cancel();
    if let Err(error) = state.forwarder.await {
        tracing::error!(target: LOG, "Dispatcher event forwarder failed: {error}");
    }
    // Waits for capacity, so a queued trigger is still drained before the stop.
    let _ = state.trigger.send(Signal::Stop).await;
    if let Err(error) = state.supervisor.await {
        tracing::error!(target: LOG, "Dispatcher supervisor failed: {error}");
    }
}

async fn forward_events(
    mut events: broadcast::Receiver<()>,
    trigger: mpsc::Sender<Signal>,
    cancel: CancellationToken,
) {
    loop {
        let event = tokio::select! {
            () = cancel.cancelled() => return,
            event = events.recv() => event,
        };
        match event {
            // A lagged receiver still means "something changed": poll.
            Ok(()) | Err(broadcast::error::RecvError::Lagged(_)) => {
                let _ = trigger.try_send(Signal::Trigger);
            }
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

async fn supervise(inner: Arc<Inner>, mut receiver: mpsc::Receiver<Signal>) {
    while let Some(signal) = receiver.recv().await {
        if signal == Signal::Stop {
            return;
        }
        if let Err(error) = inner.process_pass().await {
            tracing::error!(target: LOG, error = %error, "Dispatcher error");
        }
    }
}

impl Inner {
    async fn process_pass(&self) -> Result<(), DispatchError> {
        let poll = self
            .source
            .poll()
            .await
            .map_err(|source| DispatchError::Source {
                source_name: self.source_name.clone(),
                source,
            })?;
        let dispatched = !poll.emails.is_empty();
        if dispatched {
            self.dispatch(&poll.emails).await?;
        }
        // Advancing the cursor is valid only after every handler durably accepted
        // the batch; on failure, replay is intentional and handler dedup is the gate.
        (poll.commit)()
            .await
            .map_err(|source| DispatchError::Commit {
                source_name: self.source_name.clone(),
                source,
            })?;
        if dispatched {
            dispatch_state::save_last_dispatched_at(&self.store, self.store.clock().now_ms())
                .await
                .map_err(|source| DispatchError::Watermark {
                    source_name: self.source_name.clone(),
                    source,
                })?;
        }
        Ok(())
    }

    async fn dispatch(&self, emails: &[FetchedEmail]) -> Result<(), DispatchError> {
        let results = join_all(self.handlers.iter().map(|handler| async move {
            handler
                .handle(emails)
                .await
                .map_err(|source| DispatchError::Handler {
                    handler: handler.name().to_owned(),
                    source,
                })
        }))
        .await;
        let mut first = None;
        for error in results.into_iter().filter_map(Result::err) {
            tracing::error!(target: LOG, "{error}");
            first.get_or_insert(error);
        }
        first.map_or(Ok(()), Err)
    }
}

/// The long-lived dispatcher service for WP14 (`startEmailFeatures`): starts
/// the dispatcher over `source` with the ordered `handlers`, runs until
/// shutdown, then stops it. A failed start ends the future at ERROR so the
/// service's 30 s to 300 s restart policy retries it.
pub fn service(
    source: Arc<dyn MailSource>,
    source_name: &'static str,
    handlers: Vec<Arc<dyn EmailHandler>>,
) -> omni_runtime::BackgroundService {
    omni_runtime::BackgroundService {
        name: "EmailDispatcher",
        start: Box::new(move |ctx: omni_runtime::AppContext| {
            let source = source.clone();
            let handlers = handlers.clone();
            Box::pin(async move {
                if handlers.is_empty() {
                    tracing::info!(target: LOG, "No email pipelines active");
                    ctx.shutdown.cancelled().await;
                    return;
                }
                let dispatcher = EmailDispatcher::new(
                    ctx.store.clone(),
                    source,
                    source_name,
                    handlers,
                    ctx.tracker.clone(),
                );
                if let Err(error) = dispatcher.start().await {
                    tracing::error!(
                        target: LOG,
                        "start email transport failed: {error}"
                    );
                    return;
                }
                tracing::info!(
                    target: LOG,
                    "Started {source_name} transport with {} pipeline(s)",
                    dispatcher.handler_count()
                );
                ctx.shutdown.cancelled().await;
                dispatcher.stop().await;
            })
        }),
        retry: Some(omni_runtime::RetryPolicy {
            initial: std::time::Duration::from_secs(30),
            max: std::time::Duration::from_secs(300),
        }),
    }
}
