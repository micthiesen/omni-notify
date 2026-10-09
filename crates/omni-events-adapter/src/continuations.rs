//! Native elicitation: one legacy Executor tool call stays open across modern
//! `input_required` rounds.
//!
//! Each invocation has a total deadline (five minutes in production) and holds
//! a capacity slot until it closes. Every round hands out a fresh one-shot
//! `requestState` bound to the owner and the tool name and arguments; a reply
//! claims the state before waking the invocation, so a replay or a concurrent
//! retry can never deliver the same input twice. A restart or an expired
//! deadline fails the pending reply; the caller must start a new call.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, Weak};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Map, Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::digest::args_digest;
use crate::legacy::{
    CLOSE_TIMEOUT, ElicitAnswer, ElicitHandler, ElicitationSupport, LegacyConnector, LegacySession,
};

/// Production continuation lifetime.
pub const DEFAULT_TTL: Duration = Duration::from_secs(300);
/// Production bound on concurrently open invocations.
pub const DEFAULT_MAX_ACTIVE: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ContinuationError {
    /// Unknown, consumed, foreign or malformed continuation.
    #[error("invalid")]
    Invalid,
    /// Executor failed, the deadline passed, or the invocation was closed.
    #[error("upstream")]
    Upstream,
    /// Too many open invocations.
    #[error("capacity")]
    Capacity,
}

#[derive(Debug)]
enum Outcome {
    Input(Value),
    Result(Value),
    Error,
}

struct Pending {
    input: Value,
    resolve: oneshot::Sender<ElicitAnswer>,
}

#[derive(Default)]
struct EntryState {
    pending: Option<Pending>,
    request_state: Option<String>,
    closed: bool,
}

struct Entry {
    owner: String,
    digest: String,
    session: OnceLock<Arc<dyn LegacySession>>,
    state: Mutex<EntryState>,
    outcomes_tx: mpsc::UnboundedSender<Outcome>,
    outcomes_rx: tokio::sync::Mutex<mpsc::UnboundedReceiver<Outcome>>,
    cancel: CancellationToken,
}

impl Entry {
    fn new(owner: String, digest: String) -> Self {
        let (outcomes_tx, outcomes_rx) = mpsc::unbounded_channel();
        Self {
            owner,
            digest,
            session: OnceLock::new(),
            state: Mutex::new(EntryState::default()),
            outcomes_tx,
            outcomes_rx: tokio::sync::Mutex::new(outcomes_rx),
            cancel: CancellationToken::new(),
        }
    }

    fn state(&self) -> MutexGuard<'_, EntryState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn push(&self, outcome: Outcome) {
        // The receiver lives as long as the entry.
        let _ = self.outcomes_tx.send(outcome);
    }

    async fn next(&self) -> Outcome {
        self.outcomes_rx
            .lock()
            .await
            .recv()
            .await
            .unwrap_or(Outcome::Error)
    }
}

#[derive(Default)]
struct Registry {
    entries: HashMap<String, Arc<Entry>>,
    active: usize,
}

struct Shared {
    connector: Arc<dyn LegacyConnector>,
    ttl: Duration,
    max_active: usize,
    registry: Mutex<Registry>,
}

/// In-memory continuation manager. Cheap to clone.
#[derive(Clone)]
pub struct Continuations {
    shared: Arc<Shared>,
}

/// One modern `tools/call` (first call or reply).
#[derive(Debug, Clone)]
pub struct CallRequest {
    pub owner: String,
    pub authorization: String,
    /// The caller's `elicitation_mode` query value.
    pub mode: Option<String>,
    pub params: Map<String, Value>,
    pub support: ElicitationSupport,
}

impl Continuations {
    pub fn new(connector: Arc<dyn LegacyConnector>, ttl: Duration, max_active: usize) -> Self {
        Self {
            shared: Arc::new(Shared {
                connector,
                ttl,
                max_active,
                registry: Mutex::new(Registry::default()),
            }),
        }
    }

    /// Open invocations (for tests and diagnostics).
    pub fn active(&self) -> usize {
        self.shared.registry().active
    }

    /// Runs one round to completion even if the HTTP caller goes away, so capacity
    /// and claimed state are always settled.
    pub async fn call(
        &self,
        request: CallRequest,
    ) -> Result<Map<String, Value>, ContinuationError> {
        let shared = Arc::clone(&self.shared);
        tokio::spawn(async move { shared.call(request).await })
            .await
            .unwrap_or(Err(ContinuationError::Upstream))
    }
}

/// Interprets a reply against the pending elicitation.
fn answer(value: Option<&Value>, input: &Value) -> Option<ElicitAnswer> {
    let item = value?.as_object()?;
    match item.get("action")?.as_str()? {
        "accept" => {
            let url_mode = input
                .get("params")
                .and_then(|p| p.get("mode"))
                .and_then(Value::as_str)
                == Some("url");
            match item.get("content") {
                None if url_mode => Some(ElicitAnswer::Accept(None)),
                Some(Value::Object(content)) => Some(ElicitAnswer::Accept(Some(content.clone()))),
                _ => None,
            }
        }
        "decline" => Some(ElicitAnswer::Decline),
        "cancel" => Some(ElicitAnswer::Cancel),
        _ => None,
    }
}

fn new_request_state() -> String {
    let mut bytes = [0u8; 32];
    rand::fill(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

impl Shared {
    fn registry(&self) -> MutexGuard<'_, Registry> {
        self.registry.lock().unwrap_or_else(PoisonError::into_inner)
    }

    async fn call(
        self: Arc<Self>,
        request: CallRequest,
    ) -> Result<Map<String, Value>, ContinuationError> {
        let digest = args_digest(&request.params);
        if let Some(Value::String(state)) = request.params.get("requestState") {
            return self.reply(&request, state, &digest).await;
        }
        let params = &request.params;
        let name = match params.get("name") {
            Some(Value::String(name)) if !name.is_empty() => name.clone(),
            _ => return Err(ContinuationError::Invalid),
        };
        let arguments = match params.get("arguments") {
            None => None,
            Some(Value::Object(arguments)) => Some(arguments.clone()),
            Some(_) => return Err(ContinuationError::Invalid),
        };
        if params.contains_key("inputResponses") {
            return Err(ContinuationError::Invalid);
        }
        let deadline = Instant::now() + self.ttl;
        {
            let mut registry = self.registry();
            if registry.active >= self.max_active {
                return Err(ContinuationError::Capacity);
            }
            registry.active += 1;
        }
        let entry = Arc::new(Entry::new(request.owner.clone(), digest));
        let elicit = request
            .support
            .any()
            .then(|| elicitation_handler(Arc::downgrade(&entry)));
        let connected = self
            .connector
            .connect(
                request.authorization.clone(),
                request.mode.clone(),
                request.support,
                elicit,
            )
            .await;
        let session = match connected {
            Ok(session) => session,
            Err(error) => {
                tracing::warn!(%error, "Executor tool connection failed");
                self.registry().active -= 1;
                return Err(ContinuationError::Upstream);
            }
        };
        let _ = entry.session.set(Arc::clone(&session));
        if Instant::now() >= deadline {
            self.close(&entry).await;
            return Err(ContinuationError::Upstream);
        }
        let mut tool = Map::new();
        tool.insert("name".into(), Value::String(name));
        if let Some(arguments) = arguments {
            tool.insert("arguments".into(), Value::Object(arguments));
        }
        tokio::spawn(Arc::clone(&self).supervise(Arc::clone(&entry), session, tool, deadline));
        let first = entry.next().await;
        self.render(&entry, first).await
    }

    async fn reply(
        &self,
        request: &CallRequest,
        state: &str,
        digest: &str,
    ) -> Result<Map<String, Value>, ContinuationError> {
        let (entry, pending, response) = {
            let mut registry = self.registry();
            let entry = registry
                .entries
                .get(state)
                .cloned()
                .ok_or(ContinuationError::Invalid)?;
            let mut entry_state = entry.state();
            if entry.owner != request.owner || entry.digest != digest {
                return Err(ContinuationError::Invalid);
            }
            let input = &entry_state
                .pending
                .as_ref()
                .ok_or(ContinuationError::Invalid)?
                .input;
            let responses = match request.params.get("inputResponses") {
                Some(Value::Object(responses)) => responses,
                _ => return Err(ContinuationError::Invalid),
            };
            let response =
                answer(responses.get("elicitation"), input).ok_or(ContinuationError::Invalid)?;
            // Claim before waking the original invocation. Replay and concurrent
            // retries cannot execute the accepted input twice.
            registry.entries.remove(state);
            entry_state.request_state = None;
            let pending = entry_state
                .pending
                .take()
                .ok_or(ContinuationError::Invalid)?;
            drop(entry_state);
            (Arc::clone(&entry), pending, response)
        };
        let _ = pending.resolve.send(response);
        let outcome = entry.next().await;
        self.render(&entry, outcome).await
    }

    async fn render(
        &self,
        entry: &Arc<Entry>,
        outcome: Outcome,
    ) -> Result<Map<String, Value>, ContinuationError> {
        match outcome {
            Outcome::Result(result) => {
                self.close(entry).await;
                // A non-object result is rejected and a missing `content`
                // defaults to `[]`, as MCP clients expect.
                let Value::Object(mut result) = result else {
                    return Err(ContinuationError::Upstream);
                };
                if !result.contains_key("content") {
                    result.insert("content".into(), Value::Array(Vec::new()));
                }
                let mut rendered = Map::new();
                rendered.insert("resultType".into(), "complete".into());
                rendered.extend(result);
                Ok(rendered)
            }
            Outcome::Error => {
                self.close(entry).await;
                Err(ContinuationError::Upstream)
            }
            Outcome::Input(input) => {
                let state = new_request_state();
                {
                    let mut registry = self.registry();
                    let mut entry_state = entry.state();
                    if entry_state.closed {
                        return Err(ContinuationError::Upstream);
                    }
                    entry_state.request_state = Some(state.clone());
                    registry.entries.insert(state.clone(), Arc::clone(entry));
                }
                let mut rendered = Map::new();
                rendered.insert("resultType".into(), "input_required".into());
                rendered.insert("inputRequests".into(), json!({ "elicitation": input }));
                rendered.insert("requestState".into(), Value::String(state));
                Ok(rendered)
            }
        }
    }

    /// Marks the entry closed, releases its capacity and state, cancels a pending
    /// elicitation and closes the legacy client. Idempotent.
    async fn close(&self, entry: &Entry) {
        let pending = {
            let mut registry = self.registry();
            let mut state = entry.state();
            if state.closed {
                return;
            }
            state.closed = true;
            registry.active = registry.active.saturating_sub(1);
            if let Some(request_state) = state.request_state.take() {
                registry.entries.remove(&request_state);
            }
            state.pending.take()
        };
        entry.cancel.cancel();
        if let Some(pending) = pending {
            let _ = pending.resolve.send(ElicitAnswer::Cancel);
        }
        if let Some(session) = entry.session.get() {
            let _ = tokio::time::timeout(CLOSE_TIMEOUT, session.close()).await;
        }
    }

    async fn expire(&self, entry: &Entry) {
        if entry.state().closed {
            return;
        }
        self.close(entry).await;
        entry.push(Outcome::Error);
    }

    /// Owns the legacy tool call and the deadline.
    async fn supervise(
        self: Arc<Self>,
        entry: Arc<Entry>,
        session: Arc<dyn LegacySession>,
        tool: Map<String, Value>,
        deadline: Instant,
    ) {
        let timeout = deadline
            .saturating_duration_since(Instant::now())
            .max(Duration::from_millis(1));
        tokio::select! {
            () = entry.cancel.cancelled() => return,
            () = tokio::time::sleep_until(deadline) => {
                self.expire(&entry).await;
                return;
            }
            result = session.request("tools/call", Value::Object(tool), timeout) => {
                if !entry.state().closed {
                    entry.push(match result {
                        Ok(result) => Outcome::Result(result),
                        Err(error) => {
                            tracing::warn!(%error, "Executor tool call failed");
                            Outcome::Error
                        }
                    });
                }
            }
        }
        tokio::select! {
            () = entry.cancel.cancelled() => {}
            () = tokio::time::sleep_until(deadline) => self.expire(&entry).await,
        }
    }
}

/// Bridges Executor's `elicitation/create` into the continuation's outcomes.
fn elicitation_handler(entry: Weak<Entry>) -> ElicitHandler {
    Arc::new(move |params| {
        let entry = entry.upgrade();
        Box::pin(async move {
            let Some(entry) = entry else {
                return ElicitAnswer::Cancel;
            };
            let input = json!({"method": "elicitation/create", "params": params});
            let (resolve, answered) = oneshot::channel();
            {
                let mut state = entry.state();
                if state.closed {
                    return ElicitAnswer::Cancel;
                }
                state.pending = Some(Pending {
                    input: input.clone(),
                    resolve,
                });
            }
            entry.push(Outcome::Input(input));
            drop(entry);
            answered.await.unwrap_or(ElicitAnswer::Cancel)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_follow_the_elicitation_rules() {
        let form = json!({"method": "elicitation/create", "params": {"message": "m"}});
        let url = json!({"method": "elicitation/create", "params": {"mode": "url"}});
        let accept = json!({"action": "accept"});
        assert_eq!(answer(Some(&accept), &form), None);
        assert_eq!(
            answer(Some(&accept), &url),
            Some(ElicitAnswer::Accept(None))
        );
        assert_eq!(
            answer(
                Some(&json!({"action": "accept", "content": {"a": 1}})),
                &form
            ),
            Some(ElicitAnswer::Accept(Some(
                json!({"a": 1}).as_object().cloned().unwrap()
            )))
        );
        assert_eq!(
            answer(Some(&json!({"action": "accept", "content": []})), &url),
            None
        );
        assert_eq!(
            answer(Some(&json!({"action": "decline", "content": 5})), &form),
            Some(ElicitAnswer::Decline)
        );
        assert_eq!(
            answer(Some(&json!({"action": "cancel"})), &form),
            Some(ElicitAnswer::Cancel)
        );
        assert_eq!(answer(Some(&json!({"action": "maybe"})), &form), None);
        assert_eq!(answer(Some(&json!([])), &form), None);
        assert_eq!(answer(None, &form), None);
    }

    #[test]
    fn request_states_are_256_bit_base64url() {
        let a = new_request_state();
        assert_eq!(a.len(), 43);
        assert!(
            a.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        );
        assert_ne!(a, new_request_state());
    }
}
