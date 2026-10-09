//! A recording `EventPublisher` that imitates the MCP Events outbox's
//! receipt dedup: the first publication of a `(name, dedup_key)` returns
//! `true`, replays return `false` and are still recorded.

use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use omni_runtime::ports::{EventPublication, EventPublisher, PortError, Ports};

#[derive(Default)]
struct Inner {
    published: Vec<EventPublication>,
    receipts: HashSet<(String, String)>,
    arguments: Vec<(String, BTreeMap<String, String>)>,
    fail: bool,
}

/// Every publication, in order; cheap to clone.
#[derive(Clone, Default)]
pub struct RecordedEvents(Arc<Mutex<Inner>>);

impl RecordedEvents {
    /// A recorder already set as `ports`' event publisher.
    pub fn install(ports: &Ports) -> Self {
        let events = Self::default();
        let _ = ports.set_event_publisher(Arc::new(events.clone()));
        events
    }

    fn inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Every publication, replays included.
    pub fn published(&self) -> Vec<EventPublication> {
        self.inner().published.clone()
    }

    /// Publications that were not replays of an earlier dedup key.
    pub fn distinct(&self) -> Vec<EventPublication> {
        let mut seen = HashSet::new();
        self.published()
            .into_iter()
            .filter(|event| seen.insert((event.name, event.dedup_key.clone())))
            .collect()
    }

    /// Adds an active subscription's arguments for `active_arguments`.
    pub fn subscribe(&self, name: &str, arguments: &[(&str, &str)]) {
        let arguments = arguments
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        self.inner().arguments.push((name.to_owned(), arguments));
    }

    /// Makes every later `publish` fail.
    pub fn fail(&self) {
        self.inner().fail = true;
    }
}

impl EventPublisher for RecordedEvents {
    fn publish<'a>(
        &'a self,
        event: &'a EventPublication,
    ) -> BoxFuture<'a, Result<bool, PortError>> {
        let mut inner = self.inner();
        let result = if inner.fail {
            Err(PortError::Failed {
                message: "event outbox unavailable".to_owned(),
                transient: true,
            })
        } else {
            inner.published.push(event.clone());
            Ok(inner
                .receipts
                .insert((event.name.to_owned(), event.dedup_key.clone())))
        };
        Box::pin(async move { result })
    }

    fn active_arguments<'a>(
        &'a self,
        name: &'a str,
    ) -> BoxFuture<'a, Result<Vec<BTreeMap<String, String>>, PortError>> {
        let arguments = self
            .inner()
            .arguments
            .iter()
            .filter(|(event, _)| event == name)
            .map(|(_, args)| args.clone())
            .collect();
        Box::pin(async move { Ok(arguments) })
    }
}
