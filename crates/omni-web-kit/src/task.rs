//! Owner-scoped async work.
//!
//! [`spawn_scoped`] runs a future on the browser's microtask executor and
//! aborts it when the current reactive owner (component or effect run) is
//! cleaned up, so in-flight work never outlives its view and never writes
//! state after unmount. Dropping the aborted future runs its destructors
//! (closing EventSources, removing listeners).

use std::future::Future;
use std::time::Duration;

use futures::future::{AbortHandle, Abortable};

/// A handle that aborts its task; idempotent.
#[derive(Clone, Debug)]
pub struct TaskHandle(AbortHandle);

impl TaskHandle {
    pub fn abort(&self) {
        self.0.abort();
    }

    pub fn is_aborted(&self) -> bool {
        self.0.is_aborted()
    }
}

/// Wraps `fut` so it can be aborted; the returned future resolves to `None`
/// when aborted (the inner future is dropped at that point).
pub fn abortable<F: Future>(fut: F) -> (impl Future<Output = Option<F::Output>>, TaskHandle) {
    let (handle, registration) = AbortHandle::new_pair();
    let wrapped = Abortable::new(fut, registration);
    (async move { wrapped.await.ok() }, TaskHandle(handle))
}

/// Spawn `fut` locally; it is aborted when the current owner cleans up.
pub fn spawn_scoped(fut: impl Future<Output = ()> + 'static) -> TaskHandle {
    let (wrapped, handle) = abortable(fut);
    let cleanup = handle.clone();
    leptos::prelude::on_cleanup(move || cleanup.abort());
    leptos::task::spawn_local(async move {
        let _ = wrapped.await;
    });
    handle
}

/// Spawn `fut` locally with no owner (aborted only through the handle).
pub fn spawn_detached(fut: impl Future<Output = ()> + 'static) -> TaskHandle {
    let (wrapped, handle) = abortable(fut);
    leptos::task::spawn_local(async move {
        let _ = wrapped.await;
    });
    handle
}

/// Browser timer sleep.
pub async fn sleep(duration: Duration) {
    let millis = u32::try_from(duration.as_millis()).unwrap_or(u32::MAX);
    gloo_timers::future::TimeoutFuture::new(millis).await;
}

/// `on_cleanup` for `!Send` teardown (DOM listeners, focus restoration).
pub fn on_cleanup_local(fun: impl FnOnce() + 'static) {
    use leptos::prelude::{StoredValue, UpdateValue as _};
    let boxed: Box<dyn FnOnce()> = Box::new(fun);
    let stored = StoredValue::new_local(Some(boxed));
    leptos::prelude::on_cleanup(move || {
        if let Some(Some(fun)) = stored.try_update_value(Option::take) {
            fun();
        }
    });
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use futures::FutureExt as _;
    use futures::channel::oneshot;

    use super::*;

    struct Finalizer(Rc<Cell<bool>>);

    impl Drop for Finalizer {
        fn drop(&mut self) {
            self.0.set(true);
        }
    }

    /// `effect.spec.ts`: "interrupts in-flight browser callback work when its
    /// scope closes".
    #[test]
    fn interrupts_in_flight_callback_work_when_its_scope_closes() {
        let finalized = Rc::new(Cell::new(false));
        let updated_after_cleanup = Rc::new(Cell::new(false));
        let (started_tx, started_rx) = oneshot::channel::<()>();
        let (_never_tx, never_rx) = oneshot::channel::<()>();
        let guard = Finalizer(finalized.clone());
        let updated = updated_after_cleanup.clone();
        let (task, handle) = abortable(async move {
            let _guard = guard;
            let _ = started_tx.send(());
            let _ = never_rx.await;
            updated.set(true);
        });
        let mut task = Box::pin(task);
        assert!(task.as_mut().now_or_never().is_none());
        assert!(started_rx.now_or_never().is_some());
        handle.abort();
        assert_eq!(task.as_mut().now_or_never(), Some(None));
        assert!(finalized.get());
        assert!(!updated_after_cleanup.get());
    }
}
