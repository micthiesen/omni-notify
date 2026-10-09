//! `claude.session.turn_finished` publisher.
//!
//! Diffs the host's session list against the last state Omni recorded. It
//! polls only while a subscription for the event is active and the host is
//! online, so the host's tools stay the only way to observe sessions otherwise.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use omni_core::clock::SharedClock;
use omni_runtime::ports::ClaudeSessionNotifier;
use omni_store::cbor::Extra;
use omni_store::entity::{Entity, EntityOps, EntityWrite, UpsertOpts};
use omni_store::{Store, StoreError};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::catalog::CLAUDE_TURN_FINISHED;
use super::service::{McpEventService, PublishInput};
use crate::host::{ClaudeHost, HostCommand, HostError};
use crate::json::js_number;

const LIST_LIMIT: u32 = 100;
/// Sessions that left the running list are looked up individually, a few per pass.
const MAX_STATUS_LOOKUPS: usize = 5;
/// A session that cannot be read this many polls in a row is forgotten.
const MAX_MISSES: i64 = 10;
const WATCH_RETENTION_MS: i64 = 7 * 24 * 60 * 60_000;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(60);
/// A turn last seen this long ago is history, not news, when polling resumes.
const STALE_TURN_MS: i64 = 24 * 60 * 60_000;

/// `mcp-claude-session-watch`: the last turn state Omni saw for one session.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeSessionWatch {
    pub session_id: String,
    pub id: Option<String>,
    pub project: Option<String>,
    pub revision: f64,
    pub settled: bool,
    /// Omni began a turn at `revision`; only a later revision can finish it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started: Option<bool>,
    /// Consecutive failed lookups after the session left the running list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub misses: Option<i64>,
    pub seen_at: i64,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity for ClaudeSessionWatch {
    const NAME: &'static str = "mcp-claude-session-watch";
    type Key = String;
    fn key(&self) -> String {
        self.session_id.clone()
    }
}

/// One session summary as the host's session client reports it.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct SessionSummary {
    pub session_id: String,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub project: Option<String>,
    pub status: String,
    #[serde(default)]
    pub state: Option<String>,
    pub revision: f64,
}

impl SessionSummary {
    fn decode(raw: &Value) -> Option<Self> {
        serde_json::from_value(raw.clone()).ok()
    }
}

/// The previous observation `finished_turn` compares against.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TurnBaseline {
    pub revision: f64,
    pub settled: bool,
    pub started: bool,
}

/// Same rule as the session client's wait: idle with no turn in progress, or stopped.
pub fn is_settled(status: &str, state: Option<&str>) -> bool {
    status == "stopped" || (status != "busy" && state != Some("working"))
}

/// A finished turn is a settled session whose transcript moved past the turn
/// Omni last saw in progress, or that settled since Omni last saw it working.
/// A session seen for the first time only sets the baseline. A turn Omni just
/// started can report idle before it begins, so it needs a later revision.
pub fn finished_turn(
    previous: Option<TurnBaseline>,
    status: &str,
    state: Option<&str>,
    revision: f64,
) -> bool {
    let Some(previous) = previous else {
        return false;
    };
    if !is_settled(status, state) {
        return false;
    }
    if previous.started {
        return revision > previous.revision;
    }
    !previous.settled || revision > previous.revision
}

#[derive(Debug, thiserror::Error)]
pub enum WatcherError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Host(#[from] HostError),
    #[error("the Claude Code host returned a malformed session list")]
    MalformedList,
}

/// A turn Omni just started (`noteTurnStarted` input).
pub use omni_runtime::ports::ClaudeTurnStarted as StartedTurn;

/// Publishes `claude.session.turn_finished`; cheap to clone.
#[derive(Clone)]
pub struct ClaudeSessionWatcher {
    events: McpEventService,
    host: Arc<dyn ClaudeHost>,
    store: Store,
    clock: SharedClock,
}

impl ClaudeSessionWatcher {
    pub fn new(
        events: McpEventService,
        host: Arc<dyn ClaudeHost>,
        store: Store,
        clock: SharedClock,
    ) -> Self {
        Self {
            events,
            host,
            store,
            clock,
        }
    }

    /// Records a turn Omni just started, so a turn that ends before the next
    /// poll still produces an event.
    pub async fn note_turn_started(&self, turn: StartedTurn) -> Result<(), StoreError> {
        let row = ClaudeSessionWatch {
            session_id: turn.session_id,
            id: turn.id,
            project: turn.project,
            revision: turn.revision,
            settled: false,
            started: Some(true),
            misses: None,
            seen_at: self.clock.now_ms(),
            extra: Extra::default(),
        };
        self.upsert(row).await
    }

    async fn upsert(&self, row: ClaudeSessionWatch) -> Result<(), StoreError> {
        self.store
            .write(move |tx| tx.upsert(&row, UpsertOpts::default()))
            .await
    }

    async fn delete(&self, session_id: &str) -> Result<(), StoreError> {
        let key = session_id.to_owned();
        self.store
            .write(move |tx| tx.delete::<ClaudeSessionWatch>(&key).map(|_| ()))
            .await
    }

    pub async fn poll(&self) -> Result<(), WatcherError> {
        if !self
            .events
            .has_active_subscription(CLAUDE_TURN_FINISHED)
            .await?
        {
            return Ok(());
        }
        let link = self.host.status();
        if !link.online || link.disabled {
            return Ok(());
        }
        let mut args = Map::new();
        args.insert("all".to_owned(), Value::Bool(false));
        args.insert("limit".to_owned(), json!(LIST_LIMIT));
        let listed = self
            .host
            .execute(HostCommand::List, args, COMMAND_TIMEOUT)
            .await?;
        let sessions = listed
            .get("sessions")
            .and_then(Value::as_array)
            .ok_or(WatcherError::MalformedList)?;
        let watches: HashMap<String, ClaudeSessionWatch> = self
            .store
            .read(|docs| docs.get_all::<ClaudeSessionWatch>())
            .await?
            .into_iter()
            .map(|watch| (watch.session_id.clone(), watch))
            .collect();
        let mut seen = HashSet::new();
        for raw in sessions {
            let Some(summary) = SessionSummary::decode(raw) else {
                continue;
            };
            seen.insert(summary.session_id.clone());
            self.observe(watches.get(&summary.session_id), &summary)
                .await?;
        }
        // A stopped session leaves the running list; read the ones mid-turn,
        // least-missed first so one unreadable session cannot starve the rest.
        let mut missing: Vec<&ClaudeSessionWatch> = watches
            .values()
            .filter(|watch| !watch.settled && !seen.contains(&watch.session_id))
            .collect();
        missing.sort_by_key(|watch| watch.misses.unwrap_or(0));
        for watch in missing.into_iter().take(MAX_STATUS_LOOKUPS) {
            let mut args = Map::new();
            args.insert(
                "session".to_owned(),
                Value::String(watch.session_id.clone()),
            );
            let status = self
                .host
                .execute(HostCommand::Status, args, COMMAND_TIMEOUT)
                .await;
            let summary = status
                .as_ref()
                .ok()
                .and_then(|data| SessionSummary::decode(&Value::Object(data.clone())));
            if let Some(summary) = summary {
                self.observe(Some(watch), &summary).await?;
                continue;
            }
            let misses = watch.misses.unwrap_or(0) + 1;
            let gone = status
                .as_ref()
                .is_err_and(|error| error.code == "not_found");
            if gone || misses >= MAX_MISSES {
                self.delete(&watch.session_id).await?;
            } else {
                self.upsert(ClaudeSessionWatch {
                    misses: Some(misses),
                    ..watch.clone()
                })
                .await?;
            }
        }
        let now = self.clock.now_ms();
        for watch in watches.values() {
            if !seen.contains(&watch.session_id) && watch.seen_at < now - WATCH_RETENTION_MS {
                self.delete(&watch.session_id).await?;
            }
        }
        Ok(())
    }

    async fn observe(
        &self,
        previous: Option<&ClaudeSessionWatch>,
        summary: &SessionSummary,
    ) -> Result<(), StoreError> {
        let now = self.clock.now_ms();
        let project = summary
            .project
            .clone()
            .or_else(|| previous.and_then(|p| p.project.clone()));
        let id = summary
            .id
            .clone()
            .or_else(|| previous.and_then(|p| p.id.clone()));
        let recent = previous.is_some_and(|p| p.seen_at > now - STALE_TURN_MS);
        let baseline = previous.map(|p| TurnBaseline {
            revision: p.revision,
            settled: p.settled,
            started: p.started.unwrap_or(false),
        });
        if recent
            && finished_turn(
                baseline,
                &summary.status,
                summary.state.as_deref(),
                summary.revision,
            )
        {
            let key = format!(
                "{CLAUDE_TURN_FINISHED}:{}:{}",
                summary.session_id,
                omni_core::js::number_to_string(summary.revision)
            );
            let data = json!({
                "sessionId": summary.session_id,
                "id": id,
                "project": project,
                "status": summary.status,
                "revision": js_number(summary.revision),
            });
            self.events
                .publish(PublishInput {
                    name: CLAUDE_TURN_FINISHED.to_owned(),
                    receipt_key: omni_core::digest::sha256_hex(&key),
                    event_key: key,
                    timestamp: omni_core::js::to_iso_string(now),
                    data: data.as_object().cloned().unwrap_or_default(),
                })
                .await?;
        }
        self.upsert(ClaudeSessionWatch {
            session_id: summary.session_id.clone(),
            id,
            project,
            revision: summary.revision,
            settled: is_settled(&summary.status, summary.state.as_deref()),
            started: None,
            misses: None,
            seen_at: now,
            extra: Extra::default(),
        })
        .await
    }
}

impl ClaudeSessionNotifier for ClaudeSessionWatcher {
    fn note_turn_started<'a>(&'a self, turn: &'a StartedTurn) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            if let Err(error) = ClaudeSessionWatcher::note_turn_started(self, turn.clone()).await {
                tracing::debug!(target: "MCP:ClaudeSessions", error = %error, "Turn start not recorded");
            }
        })
    }
}
