//! The durable archive workflow (`src/email/archive/service.ts`). Every
//! mailbox mutation (MOVE, COPY, STORE `\Deleted`, `UID EXPUNGE`) is claimed
//! durably first and never repeated; lost responses are reconciled by reads.
//! One workflow runs at a time (including the receipt write after COPY).

use std::sync::Arc;

use futures::future::BoxFuture;
use omni_core::clock::SharedClock;
use omni_store::Store;

use crate::archive_store::{
    ArchiveAction, ArchiveActionError, ArchiveActionStatus as S, ArchivePatch, ArchiveReason,
    get_archive_action, list_archive_actions, list_archive_actions_for_message,
    update_archive_action,
};
use crate::ops::archive::{
    ArchiveIdentity, ArchiveLocation, ArchiveMoveResult, ArchiveReconcileResult, ArchiveSnapshot,
    ArchiveSourceRequest, ArchiveStrategy, DeletedSourceState,
};
use crate::protocol::ImapError;

type MailboxResult<'a, T> = BoxFuture<'a, Result<T, ImapError>>;

/// The mailbox side of archive actions (implemented by the IMAP transport).
pub trait ArchiveMailbox: Send + Sync {
    fn inspect<'a>(
        &'a self,
        source: &'a ArchiveSourceRequest,
    ) -> MailboxResult<'a, ArchiveSnapshot>;
    fn move_message<'a>(
        &'a self,
        source: &'a ArchiveSourceRequest,
        source_hash: &'a str,
    ) -> MailboxResult<'a, ArchiveMoveResult>;
    fn reconcile<'a>(
        &'a self,
        source: &'a ArchiveSourceRequest,
        source_hash: &'a str,
    ) -> MailboxResult<'a, ArchiveReconcileResult>;
    fn restore<'a>(
        &'a self,
        identity: &'a ArchiveIdentity,
        destination: &'a ArchiveLocation,
        source_hash: &'a str,
    ) -> MailboxResult<'a, ArchiveMoveResult>;
    fn inspect_destination<'a>(
        &'a self,
        identity: &'a ArchiveIdentity,
        destination: &'a ArchiveLocation,
    ) -> MailboxResult<'a, ArchiveSnapshot>;
    fn reconcile_restore<'a>(
        &'a self,
        identity: &'a ArchiveIdentity,
        destination: &'a ArchiveLocation,
        source_hash: &'a str,
    ) -> MailboxResult<'a, ArchiveReconcileResult>;
    fn verify<'a>(
        &'a self,
        location: &'a ArchiveLocation,
        message_id: &'a str,
        source_hash: &'a str,
        flags: &'a [String],
    ) -> MailboxResult<'a, bool>;
    /// Whether the UIDPLUS copy steps below are available.
    fn supports_copy(&self) -> bool {
        true
    }
    fn copy<'a>(
        &'a self,
        source: &'a ArchiveSourceRequest,
        target_folder: &'a str,
        snapshot: &'a ArchiveSnapshot,
    ) -> MailboxResult<'a, ArchiveLocation>;
    fn reconcile_copy<'a>(
        &'a self,
        source: &'a ArchiveSourceRequest,
        target_folder: &'a str,
        snapshot: &'a ArchiveSnapshot,
    ) -> MailboxResult<'a, ArchiveReconcileResult>;
    fn mark_deleted<'a>(
        &'a self,
        source: &'a ArchiveIdentity,
        destination: &'a ArchiveLocation,
        snapshot: &'a ArchiveSnapshot,
    ) -> MailboxResult<'a, bool>;
    fn inspect_deleted<'a>(
        &'a self,
        source: &'a ArchiveIdentity,
        destination: &'a ArchiveLocation,
        snapshot: &'a ArchiveSnapshot,
    ) -> MailboxResult<'a, DeletedSourceState>;
    fn expunge<'a>(
        &'a self,
        source: &'a ArchiveIdentity,
        destination: &'a ArchiveLocation,
        snapshot: &'a ArchiveSnapshot,
    ) -> MailboxResult<'a, bool>;
}

fn inspect_failure_reason(error: &ImapError) -> ArchiveReason {
    let message = error.to_string();
    if message.contains("does not advertise MOVE") {
        ArchiveReason::NativeMoveUnavailable
    } else if message.contains("neither MOVE nor UIDPLUS") {
        ArchiveReason::SafeMoveUnavailable
    } else if message.contains("Inbox UID is gone")
        || message.contains("Inbox source is already Deleted")
        || message.contains("UIDVALIDITY changed")
        || message.contains("different Message-ID")
    {
        ArchiveReason::SourceUnavailable
    } else {
        ArchiveReason::TransportUnavailable
    }
}

/// An older queued sibling (or any unsettled non-queued one) goes first.
fn blocks_queued_sibling(sibling: &ArchiveAction, action: &ArchiveAction) -> bool {
    if sibling.action_id == action.action_id || sibling.status.is_settled() {
        return false;
    }
    if sibling.status != S::Queued {
        return true;
    }
    sibling.created_at < action.created_at
        || (sibling.created_at == action.created_at && sibling.action_id < action.action_id)
}

fn same_location(a: &ArchiveLocation, b: &ArchiveLocation) -> bool {
    a.same(b)
}

/// The archive workflow; cheap to clone, one workflow lock per service.
#[derive(Clone)]
pub struct ArchiveService {
    store: Store,
    clock: SharedClock,
    workflow: Arc<tokio::sync::Mutex<()>>,
}

impl ArchiveService {
    pub fn new(store: Store, clock: SharedClock) -> Self {
        Self {
            store,
            clock,
            workflow: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Serializes a whole archive workflow (including receipt writes after COPY).
    pub async fn with_workflow<T>(&self, fut: impl std::future::Future<Output = T>) -> T {
        let _permit = self.workflow.lock().await;
        fut.await
    }

    async fn update(
        &self,
        action_id: &str,
        expected: S,
        status: S,
        patch: ArchivePatch,
    ) -> Result<ArchiveAction, ArchiveActionError> {
        update_archive_action(&self.store, action_id, expected, status, patch).await
    }

    /// The forward source plus verified Archive copies owned by settled siblings.
    async fn archive_source(
        &self,
        action: &ArchiveAction,
    ) -> Result<ArchiveSourceRequest, ArchiveActionError> {
        let siblings =
            list_archive_actions_for_message(&self.store, &action.identity.message_id).await?;
        let claimed_copies = siblings
            .into_iter()
            .filter(|s| s.action_id != action.action_id && s.status == S::Archived)
            .filter_map(|s| s.destination)
            .collect();
        Ok(ArchiveSourceRequest {
            identity: action.identity.clone(),
            claimed_copies,
        })
    }

    /// Processes one queued action: inspect, claim, then one MOVE (or the
    /// first UIDPLUS COPY step).
    pub async fn process(
        &self,
        action: ArchiveAction,
        mailbox: &dyn ArchiveMailbox,
    ) -> Result<ArchiveAction, ArchiveActionError> {
        if action.status != S::Queued {
            return Ok(action);
        }
        // Queueing runs outside the workflow lock, so a sibling restore may have
        // started since; wait for it. Among queued siblings the oldest proceeds.
        let siblings =
            list_archive_actions_for_message(&self.store, &action.identity.message_id).await?;
        if siblings.iter().any(|s| blocks_queued_sibling(s, &action)) {
            let now = self.clock.now_ms();
            return self
                .update(
                    &action.action_id,
                    S::Queued,
                    S::Queued,
                    ArchivePatch::next_attempt_at(now + 60_000),
                )
                .await;
        }
        let source = self.archive_source(&action).await?;
        let snapshot = match mailbox.inspect(&source).await {
            Ok(snapshot) => snapshot,
            Err(error) => {
                let reason = inspect_failure_reason(&error);
                let attempts = action.attempts + 1;
                let terminal = reason != ArchiveReason::TransportUnavailable || attempts >= 5;
                let now = self.clock.now_ms();
                let backoff = (60_000_i64
                    .saturating_mul(2_i64.saturating_pow(u32::try_from(attempts).unwrap_or(31))))
                .min(30 * 60_000);
                return self
                    .update(
                        &action.action_id,
                        S::Queued,
                        if terminal { S::Failed } else { S::Queued },
                        ArchivePatch {
                            reason: Some(Some(reason)),
                            attempts: Some(attempts),
                            next_attempt_at: Some(now + backoff),
                            ..ArchivePatch::default()
                        },
                    )
                    .await;
            }
        };
        if snapshot.strategy == Some(ArchiveStrategy::UidplusCopy) {
            let Some(target) = snapshot
                .target_folder
                .clone()
                .filter(|_| mailbox.supports_copy())
            else {
                return self
                    .update(
                        &action.action_id,
                        S::Queued,
                        S::Failed,
                        ArchivePatch::reason(ArchiveReason::TransportUnavailable),
                    )
                    .await;
            };
            let claimed = self
                .update(
                    &action.action_id,
                    S::Queued,
                    S::CopyClaimed,
                    ArchivePatch {
                        snapshot: Some(snapshot.clone()),
                        attempts: Some(action.attempts + 1),
                        reason: Some(Some(ArchiveReason::CopyUncertain)),
                        ..ArchivePatch::default()
                    },
                )
                .await?;
            let copied = mailbox.copy(&source, &target, &snapshot).await.ok();
            return self
                .advance_copy(claimed, mailbox, false, copied, true)
                .await;
        }
        let claimed = self
            .update(
                &action.action_id,
                S::Queued,
                S::Claimed,
                ArchivePatch {
                    snapshot: Some(snapshot.clone()),
                    reason: Some(Some(ArchiveReason::Uncertain)),
                    attempts: Some(action.attempts + 1),
                    ..ArchivePatch::default()
                },
            )
            .await?;
        // From here a crash may have lost the MOVE response: never retry MOVE.
        if let Ok(moved) = mailbox.move_message(&source, &snapshot.source_hash).await {
            let verified = mailbox
                .verify(
                    &moved.destination,
                    &action.identity.message_id,
                    &snapshot.source_hash,
                    &snapshot.flags,
                )
                .await;
            if matches!(verified, Ok(true)) {
                let confirmed = mailbox.reconcile(&source, &snapshot.source_hash).await;
                if let Ok(ArchiveReconcileResult::Moved {
                    destination,
                    snapshot: found,
                }) = confirmed
                    && same_location(&destination, &moved.destination)
                    && found.flags == snapshot.flags
                {
                    return self
                        .update(
                            &action.action_id,
                            S::Claimed,
                            S::Archived,
                            ArchivePatch {
                                destination: Some(moved.destination),
                                reason: Some(None),
                                ..ArchivePatch::default()
                            },
                        )
                        .await;
                }
            }
        }
        self.reconcile_claimed_archive(claimed, mailbox).await
    }

    /// Read-only reconciliation of a claimed or uncertain MOVE.
    pub async fn reconcile_claimed_archive(
        &self,
        action: ArchiveAction,
        mailbox: &dyn ArchiveMailbox,
    ) -> Result<ArchiveAction, ArchiveActionError> {
        if !matches!(action.status, S::Claimed | S::Uncertain) {
            return Ok(action);
        }
        let Some(snapshot) = action.snapshot.clone() else {
            return Ok(action);
        };
        let source = self.archive_source(&action).await?;
        match mailbox.reconcile(&source, &snapshot.source_hash).await {
            Err(_) => {
                if action.status == S::Claimed {
                    return self
                        .update(
                            &action.action_id,
                            S::Claimed,
                            S::Uncertain,
                            ArchivePatch::default(),
                        )
                        .await;
                }
                Ok(action)
            }
            Ok(ArchiveReconcileResult::Moved {
                destination,
                snapshot: found,
            }) if found.flags == snapshot.flags => {
                self.update(
                    &action.action_id,
                    action.status,
                    S::Archived,
                    ArchivePatch {
                        destination: Some(destination),
                        reason: Some(None),
                        ..ArchivePatch::default()
                    },
                )
                .await
            }
            Ok(ArchiveReconcileResult::NotMoved) => {
                self.update(
                    &action.action_id,
                    action.status,
                    S::Failed,
                    ArchivePatch::reason(ArchiveReason::VerificationFailed),
                )
                .await
            }
            Ok(_) => {
                if action.status == S::Claimed {
                    return self
                        .update(
                            &action.action_id,
                            S::Claimed,
                            S::Uncertain,
                            ArchivePatch::default(),
                        )
                        .await;
                }
                Ok(action)
            }
        }
    }

    async fn restore_unlocked(
        &self,
        action_id: &str,
        mailbox: &dyn ArchiveMailbox,
    ) -> Result<ArchiveAction, ArchiveActionError> {
        let Some(action) = get_archive_action(&self.store, action_id).await? else {
            return Err(ArchiveActionError::Rejected(
                "Archive action not found".to_owned(),
            ));
        };
        if action.status == S::Restored {
            return Ok(action);
        }
        let (Some(snapshot), Some(destination)) =
            (action.snapshot.clone(), action.destination.clone())
        else {
            return Err(ArchiveActionError::Rejected(format!(
                "Archive action cannot be restored from {}",
                action.status.as_str()
            )));
        };
        if action.status != S::Archived {
            return Err(ArchiveActionError::Rejected(format!(
                "Archive action cannot be restored from {}",
                action.status.as_str()
            )));
        }
        // Recovery matches by Message-ID and content, so one copy moves at a time.
        let siblings =
            list_archive_actions_for_message(&self.store, &action.identity.message_id).await?;
        if siblings
            .iter()
            .any(|s| s.action_id != action_id && !s.status.is_settled())
        {
            return Err(ArchiveActionError::Rejected(
                "Another copy with this Message-ID has an unresolved archive action".to_owned(),
            ));
        }
        let inspected = mailbox
            .inspect_destination(&action.identity, &destination)
            .await?;
        if inspected.source_hash != snapshot.source_hash {
            return Err(ArchiveActionError::Rejected(
                "Archived message content changed; restore refused".to_owned(),
            ));
        }
        if snapshot.is_uidplus() || inspected.is_uidplus() {
            if !mailbox.supports_copy() {
                return Err(ArchiveActionError::Rejected(
                    "UIDPLUS archive transport unavailable".to_owned(),
                ));
            }
            let restore_snapshot = ArchiveSnapshot {
                strategy: Some(ArchiveStrategy::UidplusCopy),
                target_folder: Some("INBOX".to_owned()),
                ..inspected
            };
            let claimed = self
                .update(
                    action_id,
                    S::Archived,
                    S::RestoreCopyClaimed,
                    ArchivePatch {
                        restore_snapshot: Some(restore_snapshot.clone()),
                        reason: Some(Some(ArchiveReason::CopyUncertain)),
                        ..ArchivePatch::default()
                    },
                )
                .await?;
            let source = ArchiveSourceRequest::from(
                destination.with_message_id(&action.identity.message_id),
            );
            let copied = mailbox.copy(&source, "INBOX", &restore_snapshot).await.ok();
            return self
                .advance_copy(claimed, mailbox, true, copied, true)
                .await;
        }
        let claimed = self
            .update(
                action_id,
                S::Archived,
                S::RestoreClaimed,
                ArchivePatch {
                    restore_snapshot: Some(inspected.clone()),
                    reason: Some(Some(ArchiveReason::Uncertain)),
                    ..ArchivePatch::default()
                },
            )
            .await?;
        if let Ok(moved) = mailbox
            .restore(&action.identity, &destination, &snapshot.source_hash)
            .await
        {
            let verified = mailbox
                .verify(
                    &moved.destination,
                    &action.identity.message_id,
                    &snapshot.source_hash,
                    &inspected.flags,
                )
                .await;
            if matches!(verified, Ok(true)) {
                let confirmed = mailbox
                    .reconcile_restore(&action.identity, &destination, &snapshot.source_hash)
                    .await;
                if let Ok(ArchiveReconcileResult::Moved {
                    destination: found_at,
                    snapshot: found,
                }) = confirmed
                    && same_location(&found_at, &moved.destination)
                    && found.flags == inspected.flags
                {
                    return self
                        .update(
                            action_id,
                            S::RestoreClaimed,
                            S::Restored,
                            ArchivePatch {
                                restored_location: Some(moved.destination),
                                reason: Some(None),
                                ..ArchivePatch::default()
                            },
                        )
                        .await;
                }
            }
        }
        self.reconcile_claimed_restore(claimed, mailbox).await
    }

    /// Restores only this action's recorded Archive UID to Inbox.
    pub async fn restore(
        &self,
        action_id: &str,
        mailbox: &dyn ArchiveMailbox,
    ) -> Result<ArchiveAction, ArchiveActionError> {
        self.with_workflow(self.restore_unlocked(action_id, mailbox))
            .await
    }

    /// Read-only reconciliation of a claimed or uncertain restore MOVE.
    pub async fn reconcile_claimed_restore(
        &self,
        action: ArchiveAction,
        mailbox: &dyn ArchiveMailbox,
    ) -> Result<ArchiveAction, ArchiveActionError> {
        if !matches!(action.status, S::RestoreClaimed | S::RestoreUncertain) {
            return Ok(action);
        }
        let (Some(snapshot), Some(restore_snapshot), Some(destination)) = (
            action.snapshot.clone(),
            action.restore_snapshot.clone(),
            action.destination.clone(),
        ) else {
            return Ok(action);
        };
        let result = mailbox
            .reconcile_restore(&action.identity, &destination, &snapshot.source_hash)
            .await;
        if let Ok(ArchiveReconcileResult::Moved {
            destination: restored,
            snapshot: found,
        }) = result
            && found.flags == restore_snapshot.flags
        {
            return self
                .update(
                    &action.action_id,
                    action.status,
                    S::Restored,
                    ArchivePatch {
                        restored_location: Some(restored),
                        reason: Some(None),
                        ..ArchivePatch::default()
                    },
                )
                .await;
        }
        if action.status == S::RestoreClaimed {
            return self
                .update(
                    &action.action_id,
                    S::RestoreClaimed,
                    S::RestoreUncertain,
                    ArchivePatch::default(),
                )
                .await;
        }
        Ok(action)
    }

    /// One durable step at a time. A claimed COPY/STORE/EXPUNGE is never repeated.
    pub async fn advance_copy(
        &self,
        action: ArchiveAction,
        mailbox: &dyn ArchiveMailbox,
        reverse: bool,
        mapped_destination: Option<ArchiveLocation>,
        allow_mutation: bool,
    ) -> Result<ArchiveAction, ArchiveActionError> {
        let snapshot = if reverse {
            action.restore_snapshot.clone()
        } else {
            action.snapshot.clone()
        };
        let source = if reverse {
            action
                .destination
                .as_ref()
                .map(|d| ArchiveSourceRequest::from(d.with_message_id(&action.identity.message_id)))
        } else {
            Some(self.archive_source(&action).await?)
        };
        let target = if reverse {
            Some("INBOX".to_owned())
        } else {
            snapshot.as_ref().and_then(|s| s.target_folder.clone())
        };
        let (true, Some(snapshot), Some(source), Some(target)) =
            (mailbox.supports_copy(), snapshot, source, target)
        else {
            return Err(ArchiveActionError::Rejected(
                "Incomplete UIDPLUS archive receipt".to_owned(),
            ));
        };
        let (copy_claimed, copy_verified, delete_claimed, expunge_claimed, retained, completed) =
            if reverse {
                (
                    S::RestoreCopyClaimed,
                    S::RestoreCopyVerified,
                    S::RestoreDeleteClaimed,
                    S::RestoreExpungeClaimed,
                    S::RestoreCopiedSourceRetained,
                    S::Restored,
                )
            } else {
                (
                    S::CopyClaimed,
                    S::CopyVerified,
                    S::DeleteClaimed,
                    S::ExpungeClaimed,
                    S::CopiedSourceRetained,
                    S::Archived,
                )
            };
        let location_patch = |location: ArchiveLocation| {
            if reverse {
                ArchivePatch {
                    restored_location: Some(location),
                    ..ArchivePatch::default()
                }
            } else {
                ArchivePatch {
                    destination: Some(location),
                    ..ArchivePatch::default()
                }
            }
        };
        let copied_location = |action: &ArchiveAction| {
            if reverse {
                action.restored_location.clone()
            } else {
                action.destination.clone()
            }
        };
        let id = action.action_id.clone();
        let mut current = action;

        if current.status == copy_claimed {
            let mut expected = copied_location(&current);
            if let (Some(mapped), Some(expected)) = (&mapped_destination, &expected)
                && !same_location(mapped, expected)
            {
                return Ok(current);
            }
            if let (Some(mapped), None) = (&mapped_destination, &expected) {
                // The COPYUID response is stronger than a later Message-ID search:
                // store it before reconciliation so a restart cannot accept another UID.
                current = self
                    .update(
                        &id,
                        copy_claimed,
                        copy_claimed,
                        location_patch(mapped.clone()),
                    )
                    .await?;
                expected = Some(mapped.clone());
            }
            let read = mailbox.reconcile_copy(&source, &target, &snapshot).await;
            let Ok(ArchiveReconcileResult::Moved { destination, .. }) = read else {
                return Ok(current);
            };
            if expected
                .as_ref()
                .is_some_and(|e| !same_location(e, &destination))
            {
                return Ok(current);
            }
            let mut patch = location_patch(destination);
            patch.reason = Some(None);
            current = self.update(&id, copy_claimed, copy_verified, patch).await?;
        }
        if !allow_mutation && current.status == copy_verified {
            return Ok(current);
        }
        let Some(copied) = copied_location(&current) else {
            return Ok(current);
        };
        let identity = &source.identity;

        if current.status == copy_verified {
            current = self
                .update(
                    &id,
                    copy_verified,
                    delete_claimed,
                    ArchivePatch::reason(ArchiveReason::SourceMarkUncertain),
                )
                .await?;
            // The outcome is read back below; a lost response stays delete_claimed.
            let _marked = mailbox.mark_deleted(identity, &copied, &snapshot).await;
        }
        if current.status == delete_claimed {
            let read = mailbox.inspect_deleted(identity, &copied, &snapshot).await;
            match read {
                Err(_) | Ok(DeletedSourceState::Uncertain) => return Ok(current),
                Ok(DeletedSourceState::Unmarked) => {
                    return self
                        .update(
                            &id,
                            delete_claimed,
                            retained,
                            ArchivePatch::reason(ArchiveReason::CopiedSourceRetained),
                        )
                        .await;
                }
                Ok(DeletedSourceState::Absent) => {
                    return self
                        .update(&id, delete_claimed, completed, ArchivePatch::clear_reason())
                        .await;
                }
                Ok(DeletedSourceState::Marked) => {}
            }
            if !allow_mutation {
                return Ok(current);
            }
            current = self
                .update(
                    &id,
                    delete_claimed,
                    expunge_claimed,
                    ArchivePatch::reason(ArchiveReason::SourceExpungeUncertain),
                )
                .await?;
            let _expunged = mailbox.expunge(identity, &copied, &snapshot).await;
        }
        if current.status == expunge_claimed {
            let read = mailbox.inspect_deleted(identity, &copied, &snapshot).await;
            return match read {
                Err(_) | Ok(DeletedSourceState::Uncertain) => Ok(current),
                Ok(DeletedSourceState::Absent) => {
                    self.update(
                        &id,
                        expunge_claimed,
                        completed,
                        ArchivePatch::clear_reason(),
                    )
                    .await
                }
                Ok(DeletedSourceState::Marked) => {
                    self.update(
                        &id,
                        expunge_claimed,
                        retained,
                        ArchivePatch::reason(ArchiveReason::CopiedSourceDeleted),
                    )
                    .await
                }
                Ok(DeletedSourceState::Unmarked) => {
                    self.update(
                        &id,
                        expunge_claimed,
                        retained,
                        ArchivePatch::reason(ArchiveReason::CopiedSourceRetained),
                    )
                    .await
                }
            };
        }
        if current.status == retained {
            let read = mailbox.inspect_deleted(identity, &copied, &snapshot).await;
            let next_attempt_at = self.clock.now_ms() + 5 * 60_000;
            return match read {
                Err(_) | Ok(DeletedSourceState::Uncertain) => {
                    self.update(
                        &id,
                        retained,
                        retained,
                        ArchivePatch::next_attempt_at(next_attempt_at),
                    )
                    .await
                }
                Ok(DeletedSourceState::Absent) => {
                    self.update(&id, retained, completed, ArchivePatch::clear_reason())
                        .await
                }
                Ok(state) => {
                    let reason = if state == DeletedSourceState::Marked {
                        ArchiveReason::CopiedSourceDeleted
                    } else {
                        ArchiveReason::CopiedSourceRetained
                    };
                    self.update(
                        &id,
                        retained,
                        retained,
                        ArchivePatch {
                            reason: Some(Some(reason)),
                            next_attempt_at: Some(next_attempt_at),
                            ..ArchivePatch::default()
                        },
                    )
                    .await
                }
            };
        }
        Ok(current)
    }

    /// `email_archive_status`: reconciles claimed outcomes with mailbox reads
    /// only; never a new mutation.
    pub async fn status(
        &self,
        action_id: &str,
        mailbox: Option<&dyn ArchiveMailbox>,
    ) -> Result<ArchiveAction, ArchiveActionError> {
        self.with_workflow(async {
            let Some(mut action) = get_archive_action(&self.store, action_id).await? else {
                return Err(ArchiveActionError::Rejected(
                    "Archive action not found".to_owned(),
                ));
            };
            let Some(mailbox) = mailbox else {
                return Ok(action);
            };
            if matches!(action.status, S::Claimed | S::Uncertain) {
                action = self.reconcile_claimed_archive(action, mailbox).await?;
            }
            if matches!(action.status, S::RestoreClaimed | S::RestoreUncertain) {
                action = self.reconcile_claimed_restore(action, mailbox).await?;
            }
            if matches!(
                action.status,
                S::CopyClaimed
                    | S::CopyVerified
                    | S::DeleteClaimed
                    | S::ExpungeClaimed
                    | S::CopiedSourceRetained
            ) {
                action = self
                    .advance_copy(action, mailbox, false, None, false)
                    .await?;
            }
            if matches!(
                action.status,
                S::RestoreCopyClaimed
                    | S::RestoreCopyVerified
                    | S::RestoreDeleteClaimed
                    | S::RestoreExpungeClaimed
                    | S::RestoreCopiedSourceRetained
            ) {
                action = self
                    .advance_copy(action, mailbox, true, None, false)
                    .await?;
            }
            Ok(action)
        })
        .await
    }

    /// Bounded sweep (20 actions): reconciles claimed operations without
    /// repeating them, then may advance a verified action to its next claim.
    pub async fn sweep(&self, mailbox: &dyn ArchiveMailbox) -> Result<(), ArchiveActionError> {
        let now = self.clock.now_ms();
        let due: Vec<ArchiveAction> = list_archive_actions(&self.store)
            .await?
            .into_iter()
            .filter(|a| !a.status.is_settled())
            .filter(|a| a.next_attempt_at <= now)
            .collect();
        let by_due = |a: &ArchiveAction, b: &ArchiveAction| {
            a.next_attempt_at
                .cmp(&b.next_attempt_at)
                .then(a.created_at.cmp(&b.created_at))
        };
        let is_actionable = |a: &ArchiveAction| {
            matches!(
                a.status,
                S::Queued | S::CopyVerified | S::RestoreCopyVerified
            )
        };
        let mut actionable: Vec<ArchiveAction> =
            due.iter().filter(|a| is_actionable(a)).cloned().collect();
        actionable.sort_by(by_due);
        let mut recovery: Vec<ArchiveAction> =
            due.iter().filter(|a| !is_actionable(a)).cloned().collect();
        recovery.sort_by(by_due);
        let first_actionable = actionable.len().min(10);
        let first_recovery = recovery.len().min(10);
        let mut rest: Vec<ArchiveAction> = actionable[first_actionable..]
            .iter()
            .chain(recovery[first_recovery..].iter())
            .cloned()
            .collect();
        rest.sort_by(by_due);
        let remaining = 20 - first_actionable - first_recovery;
        let mut scheduled: Vec<ArchiveAction> = actionable[..first_actionable]
            .iter()
            .chain(recovery[..first_recovery].iter())
            .cloned()
            .collect();
        scheduled.extend(rest.into_iter().take(remaining));

        for planned in scheduled {
            self.with_workflow(async {
                // A receipt read failure fails the sweep (and the task run), like TS.
                let Some(action) = get_archive_action(&self.store, &planned.action_id).await? else {
                    return Ok::<(), ArchiveActionError>(());
                };
                if action.status != planned.status {
                    return Ok(());
                }
                let status = action.status;
                let result = match status {
                    S::Queued => self.process(action, mailbox).await,
                    S::Claimed | S::Uncertain => self.reconcile_claimed_archive(action, mailbox).await,
                    S::RestoreCopyClaimed
                    | S::RestoreCopyVerified
                    | S::RestoreDeleteClaimed
                    | S::RestoreExpungeClaimed
                    | S::RestoreCopiedSourceRetained => self.advance_copy(action, mailbox, true, None, true).await,
                    S::CopyClaimed | S::CopyVerified | S::DeleteClaimed | S::ExpungeClaimed | S::CopiedSourceRetained => {
                        self.advance_copy(action, mailbox, false, None, true).await
                    }
                    _ => self.reconcile_claimed_restore(action, mailbox).await,
                };
                let unchanged = match &result {
                    Err(_) => true,
                    Ok(updated) => updated.status == status,
                };
                let retained = matches!(status, S::CopiedSourceRetained | S::RestoreCopiedSourceRetained);
                if status != S::Queued && !retained && unchanged {
                    let deferred = self
                        .update(&planned.action_id, status, status, ArchivePatch::next_attempt_at(now + 5 * 60_000))
                        .await;
                    if let Err(error) = deferred {
                        tracing::debug!(target: "EmailArchive", "Archive retry deferral skipped: {error}");
                    }
                }
                Ok(())
            })
            .await?;
        }
        Ok(())
    }
}
