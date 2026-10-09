//! `SideEffectMode::Record` support for the Arr and Observer adapters.
//!
//! In record mode every mutating request (any method other than GET) is
//! captured here and fails with a "recorded" error instead of being sent, so
//! the services stop conservatively: a reservation becomes an uncertain
//! outcome in the shadow database rather than a claimed success.

use std::sync::{Arc, Mutex};

use omni_http::SideEffectMode;

/// One mutation captured in record mode.
#[derive(Clone, Debug, PartialEq)]
pub struct RecordedMutation {
    pub method: String,
    pub url: String,
    pub body: Option<serde_json::Value>,
}

/// The side-effect mode plus the mutations recorded under it; cheap to clone.
#[derive(Clone, Debug)]
pub struct SideEffects {
    mode: SideEffectMode,
    recorded: Arc<Mutex<Vec<RecordedMutation>>>,
}

impl SideEffects {
    pub fn new(mode: SideEffectMode) -> Self {
        Self {
            mode,
            recorded: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Live mode: mutations are sent.
    pub fn live() -> Self {
        Self::new(SideEffectMode::Live)
    }

    pub fn mode(&self) -> SideEffectMode {
        self.mode
    }

    /// Records `mutation` and returns `true` when in record mode (the caller
    /// must then not send it).
    pub(crate) fn intercept(&self, mutation: RecordedMutation) -> bool {
        if self.mode != SideEffectMode::Record {
            return false;
        }
        tracing::info!(
            target: "Main:SideEffects",
            "Recorded {} {} instead of sending it",
            mutation.method,
            mutation.url
        );
        self.recorded
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(mutation);
        true
    }

    /// Mutations captured in record mode, oldest first.
    pub fn recorded(&self) -> Vec<RecordedMutation> {
        self.recorded
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}
