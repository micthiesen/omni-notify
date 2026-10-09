//! The long-poll job relay.
//!
//! The Claude Code host long-polls for jobs and posts results, so Omni never
//! connects to it. Claiming and withdrawing a job are single state transitions
//! under one lock: a job withdrawn as "nothing ran" can never be delivered, and a
//! delivered job whose result never arrives is reported as an unknown outcome,
//! never retried. Job state is in memory; a restart does not replay jobs.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use omni_core::clock::SharedClock;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::sync::{Notify, oneshot};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

/// A poll this recent keeps the host online between long-polls.
pub const DEVICE_ONLINE_WINDOW_MS: i64 = 45_000;
/// How long one poll is held open waiting for jobs.
pub const DEVICE_POLL_HOLD: Duration = Duration::from_secs(25);
/// A queued job not claimed within this window is withdrawn.
pub const DEVICE_PICKUP_TIMEOUT: Duration = Duration::from_secs(30);
/// Extra time for the host to spawn the command and post its own timeout result.
pub const RESULT_SLACK: Duration = Duration::from_secs(15);

/// The bounded commands the host's session client accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceCommand {
    Projects,
    List,
    Status,
    Read,
    Result,
    Wait,
    Start,
    Send,
    Stop,
}

impl DeviceCommand {
    pub fn as_str(self) -> &'static str {
        match self {
            DeviceCommand::Projects => "projects",
            DeviceCommand::List => "list",
            DeviceCommand::Status => "status",
            DeviceCommand::Read => "read",
            DeviceCommand::Result => "result",
            DeviceCommand::Wait => "wait",
            DeviceCommand::Start => "start",
            DeviceCommand::Send => "send",
            DeviceCommand::Stop => "stop",
        }
    }
}

/// One job handed to the host.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeviceJob {
    pub id: String,
    pub command: DeviceCommand,
    pub args: Map<String, Value>,
}

/// What a poll reports about the host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PollReport {
    pub disabled: bool,
    pub host: Option<String>,
}

/// `DeviceLinkStatus`; `configured` is always true for a running service.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceLinkStatus {
    pub configured: bool,
    pub online: bool,
    pub disabled: bool,
    pub host: Option<String>,
    pub last_seen_at: Option<String>,
    pub pending_jobs: usize,
}

/// What the host reported for one job: the client's envelope or a local failure.
#[derive(Clone, Debug, PartialEq)]
pub enum DeviceJobOutcome {
    /// The session client's JSON envelope (`Null` when the host sent no output).
    Output(Value),
    Error {
        code: String,
        message: String,
    },
}

/// A relay failure with a stable machine code (`offline`, `disabled`,
/// `not_picked_up`, `outcome_unknown`, `bad_output`, or the client's own code).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{detail} ({code})")]
pub struct DeviceLinkError {
    pub code: String,
    pub detail: String,
    pub retryable: bool,
}

impl DeviceLinkError {
    pub fn new(code: impl Into<String>, detail: impl Into<String>, retryable: bool) -> Self {
        Self {
            code: code.into(),
            detail: detail.into(),
            retryable,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EntryState {
    Queued,
    Delivered,
    Withdrawn,
}

struct Entry {
    job: DeviceJob,
    state: EntryState,
    delivered: CancellationToken,
    outcome: Option<oneshot::Sender<DeviceJobOutcome>>,
}

struct HeldPoll {
    generation: u64,
    release: CancellationToken,
}

#[derive(Default)]
struct State {
    entries: HashMap<String, Entry>,
    /// Insertion order of `entries`, so jobs are handed over in queue order.
    order: Vec<String>,
    held: Option<HeldPoll>,
    next_generation: u64,
    active_polls: usize,
    last_seen_at: Option<i64>,
    disabled: bool,
    host: Option<String>,
}

struct Inner {
    clock: SharedClock,
    state: Mutex<State>,
    wake: Notify,
}

/// Relays bounded commands to the host's `omni-link` agent; cheap to clone.
#[derive(Clone)]
pub struct DeviceLinkService {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for DeviceLinkService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceLinkService").finish_non_exhaustive()
    }
}

fn iso(ms: i64) -> String {
    omni_core::js::to_iso_string(ms)
}

/// Decrements the active-poll count when a held poll ends or is dropped.
struct ActivePoll<'a>(&'a DeviceLinkService);

impl Drop for ActivePoll<'_> {
    fn drop(&mut self) {
        let mut state = self.0.state();
        state.active_polls = state.active_polls.saturating_sub(1);
    }
}

/// Withdraws a still-queued job and forgets it when `execute` ends or is dropped.
struct JobGuard<'a> {
    service: &'a DeviceLinkService,
    id: String,
}

impl Drop for JobGuard<'_> {
    fn drop(&mut self) {
        self.service.withdraw_if_queued(&self.id);
        let mut state = self.service.state();
        state.entries.remove(&self.id);
        state.order.retain(|id| id != &self.id);
    }
}

impl DeviceLinkService {
    pub fn new(clock: SharedClock) -> Self {
        Self {
            inner: Arc::new(Inner {
                clock,
                state: Mutex::new(State::default()),
                wake: Notify::new(),
            }),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn is_online(state: &State, now: i64) -> bool {
        state.active_polls > 0
            || state
                .last_seen_at
                .is_some_and(|seen| now - seen < DEVICE_ONLINE_WINDOW_MS)
    }

    pub fn status(&self) -> DeviceLinkStatus {
        let now = self.inner.clock.now_ms();
        let state = self.state();
        DeviceLinkStatus {
            configured: true,
            online: Self::is_online(&state, now),
            disabled: state.disabled,
            host: state.host.clone(),
            last_seen_at: state.last_seen_at.map(iso),
            pending_jobs: state.entries.len(),
        }
    }

    fn mark_seen(&self, state: &mut State, report: &PollReport) {
        state.last_seen_at = Some(self.inner.clock.now_ms());
        state.disabled = report.disabled;
        state.host.clone_from(&report.host);
    }

    fn claim_queued(&self) -> Vec<DeviceJob> {
        let mut state = self.state();
        let State { entries, order, .. } = &mut *state;
        let mut jobs = Vec::new();
        for id in order.iter() {
            let Some(entry) = entries.get_mut(id) else {
                continue;
            };
            if entry.state != EntryState::Queued {
                continue;
            }
            entry.state = EntryState::Delivered;
            entry.delivered.cancel();
            jobs.push(entry.job.clone());
        }
        jobs
    }

    /// Holds one long-poll open and hands over every queued job. A newer poll
    /// releases an older one, so a dead connection left by an agent restart
    /// stops claiming jobs. Claiming happens synchronously after the last
    /// await, so a dropped poll can never claim a job it does not return.
    pub async fn poll(&self, report: PollReport) -> Vec<DeviceJob> {
        let deadline = Instant::now() + DEVICE_POLL_HOLD;
        let release = CancellationToken::new();
        let generation = {
            let mut state = self.state();
            self.mark_seen(&mut state, &report);
            state.next_generation += 1;
            let generation = state.next_generation;
            if let Some(previous) = state.held.replace(HeldPoll {
                generation,
                release: release.clone(),
            }) {
                previous.release.cancel();
            }
            generation
        };
        if report.disabled {
            tokio::select! {
                () = tokio::time::sleep_until(deadline) => {}
                () = release.cancelled() => {}
            }
            return Vec::new();
        }
        self.state().active_polls += 1;
        let active = ActivePoll(self);
        let jobs = loop {
            let notified = self.inner.wake.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let claimed = self.claim_queued();
            if !claimed.is_empty() {
                break claimed;
            }
            if Instant::now() >= deadline || release.is_cancelled() {
                break Vec::new();
            }
            tokio::select! {
                () = &mut notified => {}
                () = release.cancelled() => {}
                () = tokio::time::sleep_until(deadline) => {}
            }
        };
        drop(active);
        let mut state = self.state();
        if state
            .held
            .as_ref()
            .is_some_and(|held| held.generation == generation)
        {
            state.held = None;
        }
        self.mark_seen(&mut state, &report);
        jobs
    }

    /// Accepts a result for a delivered job; `false` means unknown, withdrawn
    /// or already answered.
    pub fn complete(&self, id: &str, outcome: DeviceJobOutcome) -> bool {
        let sender = {
            let mut state = self.state();
            match state.entries.get_mut(id) {
                Some(entry) if entry.state == EntryState::Delivered => entry.outcome.take(),
                _ => None,
            }
        };
        sender.is_some_and(|sender| sender.send(outcome).is_ok())
    }

    fn withdraw_if_queued(&self, id: &str) -> bool {
        let mut state = self.state();
        match state.entries.get_mut(id) {
            Some(entry) if entry.state == EntryState::Queued => {
                entry.state = EntryState::Withdrawn;
                true
            }
            _ => false,
        }
    }

    /// Runs one command on the host and returns the client's `data` payload.
    pub async fn execute(
        &self,
        command: DeviceCommand,
        args: Map<String, Value>,
        result_timeout: Duration,
    ) -> Result<Map<String, Value>, DeviceLinkError> {
        let now = self.inner.clock.now_ms();
        let id = omni_core::ids::uuid_v4();
        let delivered = CancellationToken::new();
        let (sender, receiver) = oneshot::channel();
        {
            let mut state = self.state();
            if !Self::is_online(&state, now) {
                let seen = state.last_seen_at.map_or_else(|| "never".to_owned(), iso);
                return Err(DeviceLinkError::new(
                    "offline",
                    format!("The Claude Code host is offline (last seen {seen}); nothing ran"),
                    true,
                ));
            }
            if state.disabled {
                return Err(DeviceLinkError::new(
                    "disabled",
                    "Session control is disabled on the Claude Code host (kill switch); nothing ran",
                    false,
                ));
            }
            state.entries.insert(
                id.clone(),
                Entry {
                    job: DeviceJob {
                        id: id.clone(),
                        command,
                        args,
                    },
                    state: EntryState::Queued,
                    delivered: delivered.clone(),
                    outcome: Some(sender),
                },
            );
            state.order.push(id.clone());
        }
        let guard = JobGuard {
            service: self,
            id: id.clone(),
        };
        self.inner.wake.notify_waiters();
        let _picked = tokio::time::timeout(DEVICE_PICKUP_TIMEOUT, delivered.cancelled()).await;
        if self.withdraw_if_queued(&id) {
            return Err(DeviceLinkError::new(
                "not_picked_up",
                "The Claude Code host did not pick up the request in time; nothing ran",
                true,
            ));
        }
        let reported = tokio::time::timeout(result_timeout + RESULT_SLACK, receiver).await;
        drop(guard);
        match reported {
            Ok(Ok(outcome)) => decode_outcome(outcome),
            Ok(Err(_)) | Err(_) => Err(DeviceLinkError::new(
                "outcome_unknown",
                "The Claude Code host accepted the request but sent no result in time; the outcome is unknown. Check the session before retrying",
                false,
            )),
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Envelope {
    Ok {
        #[serde(rename = "v")]
        _v: EnvelopeVersion,
        #[serde(rename = "ok")]
        _ok: True,
        data: Map<String, Value>,
    },
    Failed {
        #[serde(rename = "v")]
        _v: EnvelopeVersion,
        #[serde(rename = "ok")]
        _ok: False,
        error: EnvelopeError,
    },
}

#[derive(Deserialize)]
struct EnvelopeError {
    code: String,
    message: String,
    #[serde(default)]
    retryable: Option<bool>,
}

macro_rules! literal {
    ($name:ident, $ty:ty, $value:expr) => {
        struct $name;
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                if <$ty>::deserialize(d)? == $value {
                    Ok($name)
                } else {
                    Err(serde::de::Error::custom("unexpected literal"))
                }
            }
        }
    };
}

literal!(EnvelopeVersion, u8, 1);
literal!(True, bool, true);
literal!(False, bool, false);

fn decode_outcome(outcome: DeviceJobOutcome) -> Result<Map<String, Value>, DeviceLinkError> {
    match outcome {
        DeviceJobOutcome::Error { code, message } => {
            Err(DeviceLinkError::new(code, message, false))
        }
        DeviceJobOutcome::Output(output) => match serde_json::from_value::<Envelope>(output) {
            Ok(Envelope::Ok { data, .. }) => Ok(data),
            Ok(Envelope::Failed { error, .. }) => Err(DeviceLinkError::new(
                error.code,
                error.message,
                error.retryable.unwrap_or(false),
            )),
            Err(_) => Err(DeviceLinkError::new(
                "bad_output",
                "The Claude Code host returned malformed output",
                false,
            )),
        },
    }
}
