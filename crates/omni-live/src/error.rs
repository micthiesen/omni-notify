//! Live-check failures.

use omni_store::StoreError;

/// A failure that fails the live-check run (and its route).
#[derive(Debug, thiserror::Error)]
pub enum LiveError {
    /// The docstore failed. The `PersistenceError:` prefix is part of the stored
    /// run error text.
    #[error("PersistenceError: {operation} failed: {source}")]
    Persistence {
        operation: &'static str,
        #[source]
        source: StoreError,
    },
    /// A live, title, offline or viewer-record notification failed.
    #[error("Notification failed: {0}")]
    Notify(#[from] NotifyError),
}

impl LiveError {
    /// `map_err` helper naming the store operation.
    pub fn persistence(operation: &'static str) -> impl Fn(StoreError) -> LiveError {
        move |source| LiveError::Persistence { operation, source }
    }
}

/// A Pushover delivery failure.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct NotifyError {
    pub message: String,
}

impl NotifyError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}
