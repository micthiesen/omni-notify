//! The Observer repair pass (`src/observer-repair/service.ts`).
//!
//! Each issue is reserved durably before assessment. A repair executes at most
//! once per report revision; comment, resolution and notification completion
//! resume from the durable phase without repeating the repair.

use std::time::Duration;

use futures::future::BoxFuture;
use omni_alerts::PushoverError;
use omni_core::clock::SharedClock;
use omni_store::Store;
use serde_json::{Map, Value, json};

use super::agent::{RepairAction, RepairDecision};
use super::arr::ArrRepairError;
use super::persistence::{
    DeliveryState, ObserverRepairState, RepairOutcome, RepairPersistenceError, RepairPhase,
    acquire_issue, list_pending, release_issue, save_issue,
};
use crate::js_opt::JsonOpt;
use crate::observer::{ObserverClientError, ObserverIssue};

const LOG: &str = "Main:ObserverRepair";

/// New or pending issues handled per run.
pub const MAX_ISSUES_PER_RUN: usize = 5;
/// The bound on executing one prepared repair.
pub const EXECUTE_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Every failure the repair pass can see.
#[derive(Debug, thiserror::Error)]
pub enum RepairError {
    /// `ObserverRepairError`.
    #[error("{operation}: {cause}")]
    Operation { operation: String, cause: String },
    #[error(transparent)]
    Observer(#[from] ObserverClientError),
    #[error(transparent)]
    Arr(#[from] ArrRepairError),
    #[error(transparent)]
    Persistence(#[from] RepairPersistenceError),
    #[error("{0}")]
    Pushover(#[source] PushoverError),
    #[error("{0}")]
    Timeout(String),
    /// A failure that names no operation.
    #[error("{0}")]
    Other(String),
}

impl RepairError {
    pub fn operation(operation: impl Into<String>, cause: impl Into<String>) -> Self {
        RepairError::Operation {
            operation: operation.into(),
            cause: cause.into(),
        }
    }

    /// The failed operation, when the error names one.
    pub fn operation_name(&self) -> Option<&str> {
        match self {
            RepairError::Operation { operation, .. } => Some(operation),
            RepairError::Observer(e) => Some(&e.operation),
            RepairError::Arr(e) => Some(&e.operation),
            RepairError::Persistence(e) => Some(&e.operation),
            RepairError::Pushover(_) | RepairError::Timeout(_) | RepairError::Other(_) => None,
        }
    }

    fn is_definite_pushover_rejection(&self) -> bool {
        matches!(self, RepairError::Pushover(e) if e.is_definite_rejection())
    }
}

/// A planned repair: the user-facing summary and the mutation that returns the
/// accepted search command id.
pub struct PreparedRepair<'a> {
    pub summary: String,
    pub execute: BoxFuture<'a, Result<i64, RepairError>>,
}

/// The side effects the pass drives (`RepairDependencies`).
pub trait RepairDependencies: Send + Sync {
    fn list_open(&self) -> BoxFuture<'_, Result<Vec<ObserverIssue>, RepairError>>;
    fn get_issue(&self, id: i64) -> BoxFuture<'_, Result<ObserverIssue, RepairError>>;
    fn assess<'a>(
        &'a self,
        issue: &'a ObserverIssue,
    ) -> BoxFuture<'a, Result<RepairDecision, RepairError>>;
    fn prepare<'a>(
        &'a self,
        issue: &'a ObserverIssue,
        decision: &'a RepairDecision,
    ) -> BoxFuture<'a, Result<PreparedRepair<'a>, RepairError>>;
    fn comment<'a>(&'a self, id: i64, message: &'a str) -> BoxFuture<'a, Result<(), RepairError>>;
    fn resolve(&self, id: i64) -> BoxFuture<'_, Result<(), RepairError>>;
    fn send<'a>(&'a self, id: i64, message: &'a str) -> BoxFuture<'a, Result<(), RepairError>>;
}

fn repair_marker(issue_id: i64) -> String {
    format!("[Omni repair {issue_id}/")
}

/// `issueRevision`: sha256 (first 24 hex) of the JS JSON of the report scope,
/// media identity and human comments (Omni's own repair comments excluded).
pub fn issue_revision(issue: &ObserverIssue) -> String {
    let mut value = Map::new();
    value.insert("id".into(), json!(issue.id));
    value.insert("status".into(), json!(issue.status));
    value.insert("type".into(), json!(issue.issue_type));
    if !issue.problem_season.is_absent() {
        value.insert("season".into(), json!(issue.problem_season));
    }
    if !issue.problem_episode.is_absent() {
        value.insert("episode".into(), json!(issue.problem_episode));
    }
    let mut media = Map::new();
    if let Some(info) = issue.media.as_option() {
        if !info.media_type.is_absent() {
            media.insert("type".into(), json!(info.media_type));
        }
        if !info.tmdb_id.is_absent() {
            media.insert("tmdbId".into(), json!(info.tmdb_id));
        }
        if !info.tvdb_id.is_absent() {
            media.insert("tvdbId".into(), json!(info.tvdb_id));
        }
    }
    value.insert("media".into(), Value::Object(media));
    if let JsonOpt::Value(comments) = &issue.comments {
        let marker = repair_marker(issue.id);
        value.insert(
            "comments".into(),
            Value::Array(
                comments
                    .iter()
                    .filter(|c| !c.message.starts_with(&marker))
                    .map(|c| json!([c.id, c.message]))
                    .collect(),
            ),
        );
    }
    let digest =
        omni_core::digest::sha256_hex(omni_core::js::json_stringify(&Value::Object(value)));
    digest[..24].to_owned()
}

/// `failureMessage`: never copies API bodies or model output into notifications/comments.
pub fn failure_message(error: &RepairError) -> String {
    let operation = error.operation_name().unwrap_or("repair operation");
    format!(
        "Could not complete {operation}. The issue remains open for manual handling; inspect the task run before retrying."
    )
}

struct IssueRun<'a, D: ?Sized> {
    deps: &'a D,
    store: &'a Store,
    clock: &'a SharedClock,
    owner: String,
    state: ObserverRepairState,
}

impl<D: RepairDependencies + ?Sized> IssueRun<'_, D> {
    async fn save(&self) -> Result<(), RepairError> {
        save_issue(self.store, &self.state, &self.owner, self.clock.now_ms())
            .await
            .map_err(RepairError::from)
    }

    fn mark_unhandled(&mut self, message: String) {
        let state = &mut self.state;
        state.phase = RepairPhase::Unhandled;
        state.outcome = Some(RepairOutcome::Unhandled);
        state.message = Some(message);
    }

    /// Assess, plan, recheck the report, then execute once.
    async fn attempt(&mut self, issue: &ObserverIssue) -> Result<(), RepairError> {
        let issue_id = issue.id;
        let deps = self.deps;
        let decision = deps.assess(issue).await?;
        tracing::info!(
            target: LOG,
            "Observer #{issue_id}: {}: {}",
            decision.action.as_str(),
            decision.reason
        );
        if decision.action == RepairAction::CannotHandle {
            self.mark_unhandled(decision.reason.clone());
            return Ok(());
        }
        let prepared = deps.prepare(issue, &decision).await?;
        let fresh = deps.get_issue(issue_id).await?;
        if issue_revision(&fresh) != self.state.revision || fresh.updated_at != issue.updated_at {
            return Err(RepairError::operation(
                "recheck changed issue before repair",
                "Issue changed during assessment",
            ));
        }
        let state = &mut self.state;
        state.phase = RepairPhase::Executing;
        state.message = Some(prepared.summary.clone());
        self.save().await?;
        let command_id = tokio::time::timeout(EXECUTE_TIMEOUT, prepared.execute)
            .await
            .map_err(|_| RepairError::Timeout("Repair execution timed out".to_owned()))??;
        let state = &mut self.state;
        state.phase = RepairPhase::Repaired;
        state.outcome = Some(RepairOutcome::Repaired);
        state.message = Some(format!(
            "{} Automatic search accepted (command {command_id}); replacement download is not yet verified.",
            prepared.summary
        ));
        Ok(())
    }

    async fn work(
        &mut self,
        issue: &ObserverIssue,
        summaries: &mut Vec<String>,
    ) -> Result<(), RepairError> {
        let issue_id = issue.id;
        if self.state.phase == RepairPhase::Reserved {
            if issue.status != 1 {
                self.state.phase = RepairPhase::Done;
                return self.save().await;
            }
            if let Err(error) = self.attempt(issue).await {
                let message = failure_message(&error);
                tracing::warn!(target: LOG, "Observer #{issue_id}: {message}");
                self.mark_unhandled(message);
            }
            self.save().await?;
        }
        self.finish().await?;
        summaries.push(format!(
            "#{issue_id} {}",
            self.state.outcome.map_or("skipped", RepairOutcome::as_str)
        ));
        Ok(())
    }

    /// Comment, resolve and notify from the durable phase.
    async fn finish(&mut self) -> Result<(), RepairError> {
        let id = self.state.issue_id;
        let revision = self.state.revision.clone();
        if matches!(
            self.state.phase,
            RepairPhase::Repaired | RepairPhase::Commented
        ) {
            let current = self.deps.get_issue(id).await?;
            if current.status == 1 && issue_revision(&current) != revision {
                let message = format!(
                    "{} The report changed during repair; left open for review.",
                    self.state.message.as_deref().unwrap_or_default()
                );
                self.mark_unhandled(message);
                self.save().await?;
            }
        }
        if self.state.phase == RepairPhase::Repaired {
            let message = format!(
                "[Omni repair {id}/{revision}] {}",
                self.state.message.as_deref().unwrap_or_default()
            );
            let current = self.deps.get_issue(id).await?;
            if !current.comment_list().iter().any(|c| c.message == message) {
                self.deps.comment(id, &message).await?;
            }
            let verified = self.deps.get_issue(id).await?;
            if !verified.comment_list().iter().any(|c| c.message == message) {
                return Err(RepairError::operation(
                    "verify repair comment",
                    "Comment not visible",
                ));
            }
            self.state.phase = RepairPhase::Commented;
            self.save().await?;
        }
        if self.state.phase == RepairPhase::Commented {
            let current = self.deps.get_issue(id).await?;
            if current.status != 2 && issue_revision(&current) != revision {
                let message = format!(
                    "{} The report changed before resolution; left open for review.",
                    self.state.message.as_deref().unwrap_or_default()
                );
                self.mark_unhandled(message);
            } else {
                if current.status != 2 {
                    self.deps.resolve(id).await?;
                }
                if self.deps.get_issue(id).await?.status != 2 {
                    return Err(RepairError::operation(
                        "verify issue resolution",
                        "Issue remains open",
                    ));
                }
                self.state.phase = RepairPhase::Resolved;
            }
            self.save().await?;
        }
        match self.state.notification {
            DeliveryState::Pending => {
                self.state.notification = DeliveryState::Sending;
                self.save().await?;
                let verb = if self.state.outcome == Some(RepairOutcome::Repaired) {
                    "Repaired"
                } else {
                    "Needs attention"
                };
                let text = format!(
                    "{verb}: {}",
                    self.state
                        .message
                        .as_deref()
                        .unwrap_or("Repair interrupted; manual review required")
                );
                if let Err(error) = self.deps.send(id, &text).await {
                    if error.is_definite_pushover_rejection() {
                        self.state.notification = DeliveryState::Pending;
                        self.save().await?;
                    }
                    return Err(error);
                }
                self.state.notification = DeliveryState::Sent;
                self.save().await?;
            }
            DeliveryState::Sending => {
                tracing::warn!(
                    target: LOG,
                    "Observer #{id}: prior notification delivery is uncertain; not sending a duplicate"
                );
                return Err(RepairError::operation(
                    "reconcile uncertain Pushover delivery",
                    "Delivery remains uncertain; manual reconciliation required",
                ));
            }
            DeliveryState::Sent => {}
        }
        self.state.phase = RepairPhase::Done;
        self.save().await
    }
}

/// One repair pass: pending reservations first, then open issues, at most five.
pub async fn run_observer_repair<D: RepairDependencies + ?Sized>(
    deps: &D,
    store: &Store,
    clock: &SharedClock,
) -> Result<String, RepairError> {
    let pending = list_pending(store).await?;
    let open = deps.list_open().await?;
    let mut candidates: Vec<i64> = Vec::new();
    for id in pending
        .iter()
        .map(|s| s.issue_id)
        .chain(open.iter().map(|i| i.id))
    {
        if !candidates.contains(&id) {
            candidates.push(id);
        }
    }
    let mut summaries: Vec<String> = Vec::new();
    let mut incomplete: Vec<i64> = Vec::new();
    let mut handled = 0usize;
    for issue_id in candidates {
        if handled >= MAX_ISSUES_PER_RUN {
            break;
        }
        let issue = deps.get_issue(issue_id).await?;
        let revision = pending
            .iter()
            .find(|s| s.issue_id == issue_id)
            .map_or_else(|| issue_revision(&issue), |s| s.revision.clone());
        let owner = omni_core::ids::uuid_v4();
        let Some(state) = acquire_issue(store, issue_id, &revision, &owner, clock.now_ms()).await?
        else {
            continue;
        };
        handled += 1;
        let mut run = IssueRun {
            deps,
            store,
            clock,
            owner: owner.clone(),
            state,
        };
        let result = run.work(&issue, &mut summaries).await;
        if let Err(error) = release_issue(store, issue_id, &owner, clock.now_ms()).await {
            tracing::warn!(target: LOG, "Observer lease release failed: {}", error.operation);
        }
        if let Err(error) = result {
            tracing::warn!(
                target: LOG,
                "Observer #{issue_id}: completion pending ({})",
                failure_message(&error)
            );
            // Keep the durable phase; retry only unfinished comment/status delivery, never repairs.
            summaries.push(format!("#{issue_id} completion pending"));
            incomplete.push(issue_id);
        }
    }
    if !incomplete.is_empty() {
        return Err(RepairError::operation(
            format!(
                "complete Observer issues {}",
                incomplete
                    .iter()
                    .map(i64::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            "Durable completion remains pending",
        ));
    }
    Ok(if summaries.is_empty() {
        "No unhandled Observer issues".to_owned()
    } else {
        summaries.join("; ")
    })
}
