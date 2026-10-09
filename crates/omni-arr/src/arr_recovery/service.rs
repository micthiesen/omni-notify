//! The recovery pass.
//!
//! Per service: take the durable lease, observe settled failures, reconcile
//! earlier reservations against Arr's actual state, act on failures that stayed
//! unchanged for 15 minutes (reserving each mutation before its HTTP call),
//! then deliver one batched notification with its own delivery reservation.

use std::ops::Range;
use std::time::Duration;

use futures::future::BoxFuture;
use omni_core::clock::SharedClock;
use omni_store::Store;

use super::nzbget::NzbGetClient;
use super::persistence::{
    ActionPhase, NotificationState, Observation, RecoveryAction, RecoveryState, acquire_state,
    release_state, save_state,
};
use super::policy::{decide, eligible_queue_item, observation_fingerprint};
use super::types::{
    ArrCause, ArrClient, ArrKind, ArrRecoveryError, ArrResult, Decision, DecisionSource, Evidence,
    QueueItem, Target,
};
use crate::paths;

const LOG: &str = "Main:ArrRecovery";

/// A failure must persist unchanged across observations spanning this long.
pub const STUCK_GRACE_MS: i64 = 15 * 60_000;
/// A longer gap between observations starts a fresh wait.
pub const MAX_OBSERVATION_GAP_MS: i64 = 20 * 60_000;
/// Unresolved cases are assessed again after this long (or when the failure changes).
pub const ASSESSMENT_COOLDOWN_MS: i64 = 24 * 60 * 60_000;
/// New actions per service per pass.
pub const MAX_ACTIONS: usize = 60;
/// Luna assessments per service per pass.
pub const MAX_LLM_CALLS: usize = 5;
/// Minimum spacing of replacement searches for an overlapping target.
pub const SEARCH_BACKOFF_MS: i64 = 6 * 60 * 60_000;
/// Window of the three-attempt replacement budget.
pub const SEARCH_WINDOW_MS: i64 = 7 * 24 * 60 * 60_000;
/// Completed, notified actions are kept this long.
pub const HISTORY_RETENTION_MS: i64 = 30 * 24 * 60 * 60_000;
/// The bound on one service's pass.
pub const RUN_TIMEOUT: Duration = Duration::from_secs(20 * 60);
/// Pause between a manual import submission and its first verification.
pub const IMPORT_SETTLE_DELAY: Duration = Duration::from_secs(2);
/// Pushover's 1,024-character limit with margin.
const MAX_NOTIFICATION_CHARS: usize = 1000;

/// Interprets evidence the deterministic policy defers (Luna in production).
pub trait Assessor: Send + Sync {
    fn assess<'a>(&'a self, evidence: &'a Evidence) -> BoxFuture<'a, ArrResult<Decision>>;
}

/// Delivers one batched recovery notification.
pub trait Notifier: Send + Sync {
    fn send<'a>(&'a self, kind: ArrKind, message: &'a str) -> BoxFuture<'a, ArrResult<()>>;
}

/// Corroborates a no-files rejection with the download client (NZBGet).
pub trait HealthSource: Send + Sync {
    fn download_health<'a>(
        &'a self,
        kind: ArrKind,
        download_id: &'a str,
        output_path: &'a str,
    ) -> BoxFuture<'a, ArrResult<Option<String>>>;
}

impl HealthSource for NzbGetClient {
    fn download_health<'a>(
        &'a self,
        kind: ArrKind,
        download_id: &'a str,
        output_path: &'a str,
    ) -> BoxFuture<'a, ArrResult<Option<String>>> {
        Box::pin(NzbGetClient::download_health(
            self,
            kind,
            download_id,
            output_path,
        ))
    }
}

/// What one recovery pass runs against.
pub struct RecoveryContext<'a> {
    pub store: &'a Store,
    pub clock: &'a SharedClock,
    pub assessor: &'a dyn Assessor,
    pub notifier: &'a dyn Notifier,
    pub health: Option<&'a dyn HealthSource>,
    pub import_settle_delay: Duration,
}

impl RecoveryContext<'_> {
    fn now(&self) -> i64 {
        self.clock.now_ms()
    }
}

/// `observe`: extends an unchanged, recent observation or starts a new one.
pub fn observe(previous: Option<&Observation>, fingerprint: &str, now: i64) -> Observation {
    match previous {
        Some(previous)
            if previous.fingerprint == fingerprint
                && now >= previous.last_seen_at
                && now - previous.last_seen_at <= MAX_OBSERVATION_GAP_MS =>
        {
            Observation {
                last_seen_at: now,
                observations: previous.observations + i64::from(now > previous.last_seen_at),
                ..previous.clone()
            }
        }
        _ => Observation {
            fingerprint: fingerprint.to_owned(),
            first_seen_at: now,
            last_seen_at: now,
            observations: 1,
            last_assessed_at: None,
            reason: None,
        },
    }
}

/// Two observations spanning the grace period.
pub fn is_mature(observation: &Observation) -> bool {
    observation.observations >= 2
        && observation.last_seen_at - observation.first_seen_at >= STUCK_GRACE_MS
}

/// Groups queue rows by download id (a season pack is one operation), in
/// first-appearance order.
pub fn group_queue(queue: Vec<QueueItem>) -> Vec<Vec<QueueItem>> {
    let mut groups: indexmap::IndexMap<String, Vec<QueueItem>> = indexmap::IndexMap::new();
    for item in queue {
        if item.download_id.is_empty() {
            continue;
        }
        groups
            .entry(item.download_id.clone())
            .or_default()
            .push(item);
    }
    groups.into_values().collect()
}

/// Same title and (for series) at least one overlapping episode.
pub fn same_target(a: &Target, b: &Target) -> bool {
    a.id == b.id
        && (a.episode_ids.is_empty() || a.episode_ids.iter().any(|id| b.episode_ids.contains(id)))
}

/// The replacement budget: at most three attempts per overlapping target in
/// seven days, each at least six hours after the previous.
pub fn can_replace(
    actions: &[RecoveryAction],
    target: &Target,
    now: i64,
    excluding_download_id: Option<&str>,
) -> bool {
    let attempts: Vec<&RecoveryAction> = actions
        .iter()
        .filter(|action| {
            Some(action.download_id.as_str()) != excluding_download_id
                && action.decision.is_replacement()
                && same_target(&action.target, target)
                && now - action.created_at < SEARCH_WINDOW_MS
        })
        .collect();
    attempts.len() < 3
        && attempts
            .iter()
            .all(|action| now - action.created_at >= SEARCH_BACKOFF_MS)
}

/// Source files must lie inside the reported download directory, which must be
/// disjoint from the target library directory.
pub fn source_paths_confined(evidence: &Evidence) -> bool {
    let Some(output_path) = evidence
        .items
        .first()
        .and_then(|item| item.output_path.as_deref())
        .filter(|p| !p.is_empty())
    else {
        return false;
    };
    if !paths::is_absolute(output_path) || paths::normalize(output_path) == "/" {
        return false;
    }
    let library = strip_one_trailing_slash(paths::normalize(&evidence.target.path));
    let download = strip_one_trailing_slash(paths::normalize(output_path));
    // A misconfigured output path must never turn cleanup into a library deletion.
    if !paths::is_absolute(&library)
        || library == download
        || library.starts_with(&format!("{download}/"))
        || download.starts_with(&format!("{library}/"))
    {
        return false;
    }
    let prefix = format!("{}/", output_path.strip_suffix('/').unwrap_or(output_path));
    evidence.files.iter().all(|file| {
        file.size > 0.0
            && paths::is_absolute(&file.path)
            && file.path.starts_with(&prefix)
            && !paths::relative(output_path, &file.path).starts_with("..")
    })
}

fn strip_one_trailing_slash(mut value: String) -> String {
    if value.ends_with('/') {
        value.pop();
    }
    value
}

/// Reads the target, preview, grab history and (for an empty preview) the
/// download client's health for one queue group.
pub async fn gather_evidence<C: ArrClient>(
    client: &C,
    items: &[QueueItem],
    health: Option<&dyn HealthSource>,
) -> ArrResult<Evidence> {
    let first = items
        .first()
        .ok_or_else(|| ArrRecoveryError::message("read target", "queue group is empty"))?;
    let target = client.target(items).await?;
    let files = client.preview(&first.download_id).await?;
    let grabs = client.history(&first.download_id).await?;
    let download_health = match (files.is_empty(), health, first.output_path.as_deref()) {
        (true, Some(health), Some(output_path)) if !output_path.is_empty() => {
            health
                .download_health(client.kind(), &first.download_id, output_path)
                .await?
        }
        _ => None,
    };
    Ok(Evidence {
        download_health,
        kind: client.kind(),
        items: items.to_vec(),
        target,
        files,
        grabs,
    })
}

/// Evidence key equality: target, preview, grabs and health unchanged.
fn same_evidence(a: &Evidence, b: &Evidence) -> bool {
    a.target == b.target
        && a.files == b.files
        && a.grabs == b.grabs
        && a.download_health == b.download_health
}

/// The grouped notification text for `actions`.
pub fn notification_message<'a>(actions: impl IntoIterator<Item = &'a RecoveryAction>) -> String {
    let mut groups: indexmap::IndexMap<String, usize> = indexmap::IndexMap::new();
    for action in actions {
        let verb = if action.phase != ActionPhase::Done {
            "Needs inspection"
        } else {
            match &action.decision {
                Decision::Import { .. } => "Imported",
                Decision::Remove { replace: true, .. } if action.command_id.is_some() => {
                    "Removed; replacement search requested"
                }
                Decision::Remove { replace: true, .. } => "Removed; replacement already queued",
                _ => "Removed redundant download",
            }
        };
        let title = omni_core::js::utf16_slice(&action.target.title, 0, 200);
        let luna = if action.decision.source() == DecisionSource::Llm {
            " (Luna)"
        } else {
            ""
        };
        *groups.entry(format!("{verb}: {title}{luna}")).or_insert(0) += 1;
    }
    groups
        .into_iter()
        .map(|(key, count)| {
            if count > 1 {
                format!("{key} ({count} downloads)")
            } else {
                key
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Contiguous batches whose messages stay within Pushover's limit; an
/// oversized single action still gets its own batch, so none is dropped.
fn batch_ranges(actions: &[&RecoveryAction]) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut start = 0;
    for end in 0..actions.len() {
        if end > start
            && omni_core::js::utf16_len(&notification_message(actions[start..=end].iter().copied()))
                > MAX_NOTIFICATION_CHARS
        {
            ranges.push(start..end);
            start = end;
        }
    }
    if start < actions.len() {
        ranges.push(start..actions.len());
    }
    ranges
}

pub fn notification_batches(actions: &[RecoveryAction]) -> Vec<Vec<RecoveryAction>> {
    let refs: Vec<&RecoveryAction> = actions.iter().collect();
    batch_ranges(&refs)
        .into_iter()
        .map(|range| actions[range].to_vec())
        .collect()
}

/// One recovery pass over every configured service. Fails (after running every
/// service) when any service failed.
pub async fn run_recovery<C: ArrClient>(
    clients: &[C],
    cx: &RecoveryContext<'_>,
) -> ArrResult<String> {
    let mut summaries: Vec<String> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    for client in clients {
        match run_client(client, cx).await {
            Ok(summary) => summaries.push(summary),
            Err(error) => {
                let message = error.to_string();
                tracing::warn!(target: LOG, error = %message, "Arr recovery failed for {}", client.kind());
                errors.push(format!("{}: {message}", client.kind()));
            }
        }
    }
    if !errors.is_empty() {
        summaries.extend(errors);
        return Err(ArrRecoveryError::message(
            "recover Arr queues",
            summaries.join("; "),
        ));
    }
    Ok(summaries.join("; "))
}

async fn run_client<C: ArrClient>(client: &C, cx: &RecoveryContext<'_>) -> ArrResult<String> {
    let kind = client.kind();
    let owner = omni_core::ids::uuid_v4();
    let Some(state) = acquire_state(cx.store, kind, &owner, cx.now()).await? else {
        return Ok(format!("{kind}: another recovery run holds the lease"));
    };
    let mut pass = ClientPass {
        client,
        cx,
        owner: owner.clone(),
        state,
        failures: Vec::new(),
    };
    let result = match tokio::time::timeout(RUN_TIMEOUT, pass.work()).await {
        Ok(Ok(summary)) => Ok(summary),
        Ok(Err(error)) => Err(ArrRecoveryError::wrap("recover queue", error)),
        Err(_) => Err(ArrRecoveryError::new(
            "recover queue",
            ArrCause::Timeout(RUN_TIMEOUT.as_secs()),
        )),
    };
    if let Err(error) = release_state(cx.store, kind, &owner, cx.now()).await {
        tracing::warn!(target: LOG, error = %error, "Could not release Arr recovery lease");
    }
    result
}

/// One service's pass, holding the lease.
struct ClientPass<'a, C> {
    client: &'a C,
    cx: &'a RecoveryContext<'a>,
    owner: String,
    state: RecoveryState,
    failures: Vec<String>,
}

impl<C: ArrClient> ClientPass<'_, C> {
    fn kind(&self) -> ArrKind {
        self.client.kind()
    }

    async fn save(&self) -> ArrResult<()> {
        save_state(self.cx.store, &self.state, &self.owner, self.cx.now()).await
    }

    fn action_mut(&mut self, index: usize) -> ArrResult<&mut RecoveryAction> {
        self.state.actions.get_mut(index).ok_or_else(|| {
            ArrRecoveryError::message("reconcile action", "action index out of range")
        })
    }

    async fn work(&mut self) -> ArrResult<String> {
        let kind = self.kind();
        let queue = self.client.queue().await?;
        let groups = group_queue(queue);
        let now = self.cx.now();
        let eligible: Vec<Vec<QueueItem>> = groups
            .into_iter()
            .filter(|items| items.iter().all(eligible_queue_item))
            .collect();
        let state = &mut self.state;
        state
            .observations
            .retain(|id, _| eligible.iter().any(|items| items[0].download_id == *id));
        // Retain all incomplete attempts and notification reservations, plus a month of completed history.
        state.actions.retain(|action| {
            action.phase != ActionPhase::Done
                || action.notification != NotificationState::Sent
                || now - action.created_at < HISTORY_RETENTION_MS
        });
        for items in &eligible {
            let id = items[0].download_id.clone();
            let next = observe(
                state.observations.get(&id),
                &observation_fingerprint(items),
                now,
            );
            state.observations.insert(id, next);
        }
        self.save().await?;

        let mut acted = 0usize;
        let mut llm_calls = 0usize;
        let unfinished: Vec<usize> = self
            .state
            .actions
            .iter()
            .enumerate()
            .filter(|(_, action)| action.phase != ActionPhase::Done)
            .map(|(index, _)| index)
            .collect();
        for index in unfinished {
            if let Err(error) = self.reconcile(index).await {
                let message = error.to_string();
                self.action_mut(index)?.error = Some(message.clone());
                self.failures.push(message);
                self.save().await?;
            }
        }

        for items in &eligible {
            if acted >= MAX_ACTIONS {
                break;
            }
            let id = &items[0].download_id;
            let Some(observation) = self.state.observations.get(id) else {
                continue;
            };
            if !is_mature(observation)
                || self
                    .state
                    .actions
                    .iter()
                    .any(|action| action.download_id == *id)
            {
                continue;
            }
            if observation
                .last_assessed_at
                .is_some_and(|at| now - at < ASSESSMENT_COOLDOWN_MS)
            {
                continue;
            }
            if let Err(error) = self
                .assess_and_act(items, now, &mut acted, &mut llm_calls)
                .await
            {
                let message = error.to_string();
                tracing::warn!(
                    target: LOG,
                    error = %message,
                    "{kind}: recovery deferred for {}",
                    items[0].title
                );
                self.failures.push(message);
            }
        }

        self.deliver_notifications().await?;
        let awaiting = self
            .state
            .actions
            .iter()
            .filter(|action| action.phase != ActionPhase::Done)
            .count();
        let summary = format!(
            "{kind}: {acted} action(s), {awaiting} awaiting verification, {} stuck candidate(s), {llm_calls} Luna assessment(s)",
            eligible.len()
        );
        tracing::info!(target: LOG, "{summary}");
        if !self.failures.is_empty() {
            let cause = self
                .failures
                .iter()
                .take(5)
                .cloned()
                .collect::<Vec<_>>()
                .join("; ");
            return Err(ArrRecoveryError::message(summary, cause));
        }
        Ok(summary)
    }

    async fn assess_and_act(
        &mut self,
        items: &[QueueItem],
        now: i64,
        acted: &mut usize,
        llm_calls: &mut usize,
    ) -> ArrResult<()> {
        let kind = self.kind();
        let id = items[0].download_id.clone();
        let evidence = gather_evidence(self.client, items, self.cx.health).await?;
        let mut decision = decide(&evidence);
        if matches!(decision, Decision::Defer { .. }) {
            if *llm_calls >= MAX_LLM_CALLS {
                return Ok(());
            }
            *llm_calls += 1;
            decision = self.cx.assessor.assess(&evidence).await?;
        }
        if decision.is_replacement()
            && !can_replace(&self.state.actions, &evidence.target, now, None)
        {
            decision = Decision::defer(
                "Replacement search budget/backoff reached",
                DecisionSource::Rules,
            );
        }
        let fingerprint = match self.state.observations.get_mut(&id) {
            Some(observation) => {
                observation.last_assessed_at = Some(now);
                observation.reason = Some(decision.reason().to_owned());
                observation.fingerprint.clone()
            }
            None => return Ok(()),
        };
        self.save().await?;
        tracing::info!(
            target: LOG,
            "{kind}: {} {}: {}",
            decision.action(),
            items[0].title,
            decision.reason()
        );
        if matches!(decision, Decision::Defer { .. }) {
            return Ok(());
        }
        if !source_paths_confined(&evidence) {
            return Err(ArrRecoveryError::message(
                "validate download paths",
                "Missing, empty, or unconfined source path",
            ));
        }
        // Normal Arr work or a human may have resolved this during assessment.
        let fresh_items: Vec<QueueItem> = self
            .client
            .queue()
            .await?
            .into_iter()
            .filter(|item| item.download_id == id)
            .collect();
        if fresh_items.len() != items.len()
            || !fresh_items.iter().all(eligible_queue_item)
            || observation_fingerprint(&fresh_items) != fingerprint
        {
            self.state.observations.shift_remove(&id);
            return self.save().await;
        }
        let fresh = gather_evidence(self.client, &fresh_items, self.cx.health).await?;
        if !same_evidence(&fresh, &evidence) {
            self.state.observations.shift_remove(&id);
            return self.save().await;
        }
        let output_path = items[0].output_path.clone().unwrap_or_default();
        self.state.actions.push(RecoveryAction {
            download_id: id.clone(),
            title: items[0].title.clone(),
            target: evidence.target.clone(),
            files: evidence.files.clone(),
            output_path,
            decision: decision.clone(),
            phase: ActionPhase::Reserved,
            created_at: now,
            updated_at: now,
            command_id: None,
            error: None,
            notification: NotificationState::Pending,
        });
        let index = self.state.actions.len() - 1;
        self.save().await?;
        *acted += 1;
        if let Err(error) = self
            .mutate(index, &decision, &id, &evidence, fresh_items[0].id)
            .await
        {
            let message = error.to_string();
            let action = self.action_mut(index)?;
            action.error = Some(message.clone());
            if action.phase == ActionPhase::Reserved {
                action.phase = ActionPhase::Uncertain;
            }
            self.failures.push(message);
            self.save().await?;
        }
        Ok(())
    }

    /// The reserved mutation, then its first reconciliation.
    async fn mutate(
        &mut self,
        index: usize,
        decision: &Decision,
        download_id: &str,
        evidence: &Evidence,
        queue_id: i64,
    ) -> ArrResult<()> {
        match decision {
            Decision::Import { .. } => {
                let command_id = self
                    .client
                    .import_files(download_id, &evidence.files)
                    .await?;
                let action = self.action_mut(index)?;
                action.command_id = Some(command_id);
                action.phase = ActionPhase::Submitted;
                self.save().await?;
                tokio::time::sleep(self.cx.import_settle_delay).await;
            }
            Decision::Remove { replace, .. } => {
                self.client.remove(queue_id, *replace).await?;
                self.action_mut(index)?.phase = ActionPhase::Removed;
                self.save().await?;
            }
            Decision::Defer { .. } => return Ok(()),
        }
        self.reconcile(index).await
    }

    /// Advances one reserved action from Arr's actual state. Never resubmits
    /// an import or a search whose outcome is unknown.
    async fn reconcile(&mut self, index: usize) -> ArrResult<()> {
        let action = self.action_mut(index)?.clone();
        let replace = match &action.decision {
            Decision::Defer { .. } => return Ok(()),
            Decision::Import { .. } => return self.reconcile_import(index, &action).await,
            Decision::Remove { replace, .. } => *replace,
        };
        let queue = self.client.queue().await?;
        if queue
            .iter()
            .any(|item| item.download_id == action.download_id)
            || !self.client.verify_removed(&action.output_path).await?
        {
            self.action_mut(index)?.error =
                Some("Download removal/file deletion not yet verified".to_owned());
            return self.save().await;
        }
        if !replace {
            let now = self.cx.now();
            let current = self.action_mut(index)?;
            current.phase = ActionPhase::Done;
            current.error = None;
            current.updated_at = now;
            return self.save().await;
        }
        if action.phase == ActionPhase::Searching
            || (action.phase == ActionPhase::Uncertain && action.command_id.is_some())
        {
            // The command id proves acceptance. An unknown submission is held, never blindly repeated.
            let Some(command_id) = action.command_id else {
                return Ok(());
            };
            let command = self.client.command(command_id).await?;
            let current = self.action_mut(index)?;
            if command.status == "failed" || command.status == "aborted" {
                current.phase = ActionPhase::Uncertain;
                current.error = Some(format!("Replacement search {}", command.status));
            } else {
                current.phase = ActionPhase::Done;
                current.error = None;
            }
            return self.save().await;
        }
        // Refresh monitoring and file presence after removal. Search only still-missing monitored targets.
        let refs: Vec<QueueItem> = if action.target.episode_ids.is_empty() {
            vec![QueueItem::target_ref(None, None, Some(action.target.id))]
        } else {
            action
                .target
                .episode_ids
                .iter()
                .map(|episode_id| {
                    QueueItem::target_ref(Some(action.target.id), Some(*episode_id), None)
                })
                .collect()
        };
        let fresh_target = self.client.target(&refs).await?;
        let missing: Vec<_> = fresh_target
            .episodes
            .iter()
            .filter(|episode| !episode.has_file && episode.monitored)
            .cloned()
            .collect();
        let sonarr = self.kind() == ArrKind::Sonarr;
        if !fresh_target.monitored || fresh_target.has_file || (sonarr && missing.is_empty()) {
            let current = self.action_mut(index)?;
            current.decision = Decision::Remove {
                reason: "Removed; target already satisfied or unmonitored".to_owned(),
                source: current.decision.source(),
                replace: false,
            };
            current.phase = ActionPhase::Done;
            return self.save().await;
        }
        let search_target = if sonarr {
            Target {
                episode_ids: missing.iter().map(|episode| episode.id).collect(),
                episodes: missing,
                ..fresh_target.clone()
            }
        } else {
            fresh_target.clone()
        };
        let replacement_queued = queue.iter().any(|item| {
            item.download_id != action.download_id
                && if sonarr {
                    item.series_id == Some(fresh_target.id)
                        && search_target
                            .episode_ids
                            .contains(&item.episode_id.unwrap_or(-1))
                } else {
                    item.movie_id == Some(fresh_target.id)
                }
        });
        if replacement_queued {
            let current = self.action_mut(index)?;
            current.phase = ActionPhase::Done;
            current.error = None;
            return self.save().await;
        }
        let now = self.cx.now();
        if !can_replace(
            &self.state.actions,
            &search_target,
            now,
            Some(&action.download_id),
        ) {
            return Ok(());
        }
        let current = self.action_mut(index)?;
        current.phase = ActionPhase::Searching;
        current.command_id = None;
        self.save().await?;
        let command_id = self.client.search(&search_target).await?;
        self.action_mut(index)?.command_id = Some(command_id);
        self.save().await?;
        let current = self.action_mut(index)?;
        current.phase = ActionPhase::Done;
        current.error = None;
        current.updated_at = now;
        self.save().await
    }

    async fn reconcile_import(&mut self, index: usize, action: &RecoveryAction) -> ArrResult<()> {
        if self
            .client
            .verify_imported(&action.target, &action.files)
            .await?
        {
            let now = self.cx.now();
            let current = self.action_mut(index)?;
            current.phase = ActionPhase::Done;
            current.error = None;
            current.updated_at = now;
            return self.save().await;
        }
        let error = match action.command_id {
            Some(command_id) => {
                let command = self.client.command(command_id).await?;
                if command.status == "queued" || command.status == "started" {
                    return Ok(());
                }
                format!(
                    "Import command {}; expected files not verified",
                    command.status
                )
            }
            None => "Import submission outcome unknown; not submitting it twice".to_owned(),
        };
        let current = self.action_mut(index)?;
        current.error = Some(error);
        current.phase = ActionPhase::Uncertain;
        self.save().await
    }

    /// Sends pending notifications in batches, reserving each batch first. A
    /// confirmed 4xx rejection is released for the next run; any other failure
    /// stays reserved because acceptance cannot be disproved.
    async fn deliver_notifications(&mut self) -> ArrResult<()> {
        let pending: Vec<usize> = self
            .state
            .actions
            .iter()
            .enumerate()
            .filter(|(_, action)| {
                matches!(action.phase, ActionPhase::Done | ActionPhase::Uncertain)
                    && action.notification == NotificationState::Pending
            })
            .map(|(index, _)| index)
            .collect();
        let ranges = {
            let refs: Vec<&RecoveryAction> = pending
                .iter()
                .map(|index| &self.state.actions[*index])
                .collect();
            batch_ranges(&refs)
        };
        for range in ranges {
            let batch = &pending[range];
            let message =
                notification_message(batch.iter().map(|index| &self.state.actions[*index]));
            self.set_notification(batch, NotificationState::Sending);
            self.save().await?;
            match self.cx.notifier.send(self.kind(), &message).await {
                Ok(()) => {
                    self.set_notification(batch, NotificationState::Sent);
                    self.save().await?;
                }
                Err(error) => {
                    if error.is_definite_pushover_rejection() {
                        self.set_notification(batch, NotificationState::Pending);
                    }
                    self.save().await?;
                    return Err(error);
                }
            }
        }
        Ok(())
    }

    fn set_notification(&mut self, batch: &[usize], state: NotificationState) {
        for index in batch {
            if let Some(action) = self.state.actions.get_mut(*index) {
                action.notification = state;
            }
        }
    }
}
