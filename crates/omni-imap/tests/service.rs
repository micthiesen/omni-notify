//! Durable email archive actions. Each case runs on its own temporary store;
//! the mailbox is a scripted [`ArchiveMailbox`].
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};

use futures::FutureExt as _;
use futures::future::BoxFuture;
use omni_core::clock::{SharedClock, TestClock};
use omni_imap::archive_service::{ArchiveMailbox, ArchiveService};
use omni_imap::archive_store::{
    ArchiveAction, ArchiveActionStatus as S, ArchivePatch, ArchiveReason,
    archive_auto_read_protection, get_archive_action, history_key_for, is_archive_action_message,
    queue_archive_action, update_archive_action,
};
use omni_imap::ops::archive::{
    ArchiveIdentity, ArchiveLocation, ArchiveMoveResult, ArchiveReconcileResult, ArchiveSnapshot,
    ArchiveSourceRequest, ArchiveStrategy, DeletedSourceState,
};
use omni_imap::protocol::ImapError;
use omni_store::DocWrite as _;
use omni_testkit::TestStore;
use tokio::sync::Notify;

const NOW: i64 = 1_790_769_600_000;

#[derive(Clone, Debug)]
enum Call {
    Inspect(ArchiveSourceRequest),
    Move(ArchiveSourceRequest),
    Reconcile(ArchiveSourceRequest, String),
    Restore,
    InspectDestination,
    ReconcileRestore,
    Verify(ArchiveLocation, String, String, Vec<String>),
    Copy(ArchiveSourceRequest, String, ArchiveSnapshot),
    ReconcileCopy,
    Mark(ArchiveIdentity),
    InspectDeleted,
    Expunge(ArchiveIdentity, ArchiveLocation, ArchiveSnapshot),
}

type Reply<T> = Arc<dyn Fn(&Call) -> BoxFuture<'static, Result<T, ImapError>> + Send + Sync>;

fn ok<T: Clone + Send + Sync + 'static>(value: T) -> Reply<T> {
    Arc::new(move |_| {
        let value = value.clone();
        async move { Ok(value) }.boxed()
    })
}

fn fail<T: Send + 'static>(message: &'static str) -> Reply<T> {
    Arc::new(move |_| async move { Err(ImapError::new("mock", message)) }.boxed())
}

fn snapshot(flags: &[&str]) -> ArchiveSnapshot {
    ArchiveSnapshot {
        source_hash: "abc".to_owned(),
        flags: flags.iter().map(|f| (*f).to_owned()).collect(),
        strategy: None,
        target_folder: None,
    }
}

fn uidplus(flags: &[&str], target: &str) -> ArchiveSnapshot {
    ArchiveSnapshot {
        strategy: Some(ArchiveStrategy::UidplusCopy),
        target_folder: Some(target.to_owned()),
        ..snapshot(flags)
    }
}

fn loc(folder: &str, uv: &str, uid: u32) -> ArchiveLocation {
    ArchiveLocation {
        folder: folder.to_owned(),
        uid_validity: uv.to_owned(),
        uid,
    }
}

fn moved(destination: ArchiveLocation, flags: &[&str]) -> ArchiveReconcileResult {
    ArchiveReconcileResult::Moved {
        destination,
        snapshot: snapshot(flags),
    }
}

#[derive(Clone)]
struct Mock {
    calls: Arc<Mutex<Vec<Call>>>,
    copy_supported: bool,
    inspect: Reply<ArchiveSnapshot>,
    move_message: Reply<ArchiveMoveResult>,
    reconcile: Reply<ArchiveReconcileResult>,
    restore: Reply<ArchiveMoveResult>,
    inspect_destination: Reply<ArchiveSnapshot>,
    reconcile_restore: Reply<ArchiveReconcileResult>,
    verify: Reply<bool>,
    copy: Reply<ArchiveLocation>,
    reconcile_copy: Reply<ArchiveReconcileResult>,
    mark: Reply<bool>,
    inspect_deleted: Reply<DeletedSourceState>,
    expunge: Reply<bool>,
}

/// The default scripted transport.
fn transport() -> Mock {
    Mock {
        calls: Arc::new(Mutex::new(Vec::new())),
        copy_supported: false,
        inspect: ok(snapshot(&["\\Flagged"])),
        move_message: ok(ArchiveMoveResult {
            destination: loc("Archive", "20", 12),
            snapshot: snapshot(&["\\Flagged"]),
        }),
        reconcile: ok(moved(loc("Archive", "20", 12), &["\\Flagged"])),
        restore: ok(ArchiveMoveResult {
            destination: loc("INBOX", "10", 8),
            snapshot: snapshot(&["\\Seen"]),
        }),
        inspect_destination: ok(snapshot(&["\\Seen"])),
        reconcile_restore: ok(moved(loc("INBOX", "10", 8), &["\\Seen"])),
        verify: ok(true),
        copy: fail("copy not scripted"),
        reconcile_copy: ok(ArchiveReconcileResult::Uncertain),
        mark: ok(true),
        inspect_deleted: ok(DeletedSourceState::Uncertain),
        expunge: ok(true),
    }
}

impl Mock {
    fn with_copy(mut self) -> Self {
        self.copy_supported = true;
        self
    }

    fn record(&self, call: Call) -> Call {
        self.calls.lock().unwrap().push(call.clone());
        call
    }

    fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }

    fn count(&self, f: impl Fn(&Call) -> bool) -> usize {
        self.calls().iter().filter(|c| f(c)).count()
    }

    fn count_for(&self, message_id: &str, f: impl Fn(&Call) -> Option<String>) -> usize {
        self.calls()
            .iter()
            .filter(|c| f(c).as_deref() == Some(message_id))
            .count()
    }
}

impl ArchiveMailbox for Mock {
    fn inspect<'a>(
        &'a self,
        source: &'a ArchiveSourceRequest,
    ) -> BoxFuture<'a, Result<ArchiveSnapshot, ImapError>> {
        let call = self.record(Call::Inspect(source.clone()));
        (self.inspect)(&call)
    }
    fn move_message<'a>(
        &'a self,
        source: &'a ArchiveSourceRequest,
        _hash: &'a str,
    ) -> BoxFuture<'a, Result<ArchiveMoveResult, ImapError>> {
        let call = self.record(Call::Move(source.clone()));
        (self.move_message)(&call)
    }
    fn reconcile<'a>(
        &'a self,
        source: &'a ArchiveSourceRequest,
        hash: &'a str,
    ) -> BoxFuture<'a, Result<ArchiveReconcileResult, ImapError>> {
        let call = self.record(Call::Reconcile(source.clone(), hash.to_owned()));
        (self.reconcile)(&call)
    }
    fn restore<'a>(
        &'a self,
        _identity: &'a ArchiveIdentity,
        _destination: &'a ArchiveLocation,
        _hash: &'a str,
    ) -> BoxFuture<'a, Result<ArchiveMoveResult, ImapError>> {
        let call = self.record(Call::Restore);
        (self.restore)(&call)
    }
    fn inspect_destination<'a>(
        &'a self,
        _identity: &'a ArchiveIdentity,
        _destination: &'a ArchiveLocation,
    ) -> BoxFuture<'a, Result<ArchiveSnapshot, ImapError>> {
        let call = self.record(Call::InspectDestination);
        (self.inspect_destination)(&call)
    }
    fn reconcile_restore<'a>(
        &'a self,
        _identity: &'a ArchiveIdentity,
        _destination: &'a ArchiveLocation,
        _hash: &'a str,
    ) -> BoxFuture<'a, Result<ArchiveReconcileResult, ImapError>> {
        let call = self.record(Call::ReconcileRestore);
        (self.reconcile_restore)(&call)
    }
    fn verify<'a>(
        &'a self,
        location: &'a ArchiveLocation,
        message_id: &'a str,
        hash: &'a str,
        flags: &'a [String],
    ) -> BoxFuture<'a, Result<bool, ImapError>> {
        let call = self.record(Call::Verify(
            location.clone(),
            message_id.to_owned(),
            hash.to_owned(),
            flags.to_vec(),
        ));
        (self.verify)(&call)
    }
    fn supports_copy(&self) -> bool {
        self.copy_supported
    }
    fn copy<'a>(
        &'a self,
        source: &'a ArchiveSourceRequest,
        target: &'a str,
        snapshot: &'a ArchiveSnapshot,
    ) -> BoxFuture<'a, Result<ArchiveLocation, ImapError>> {
        let call = self.record(Call::Copy(
            source.clone(),
            target.to_owned(),
            snapshot.clone(),
        ));
        (self.copy)(&call)
    }
    fn reconcile_copy<'a>(
        &'a self,
        _source: &'a ArchiveSourceRequest,
        _target: &'a str,
        _snapshot: &'a ArchiveSnapshot,
    ) -> BoxFuture<'a, Result<ArchiveReconcileResult, ImapError>> {
        let call = self.record(Call::ReconcileCopy);
        (self.reconcile_copy)(&call)
    }
    fn mark_deleted<'a>(
        &'a self,
        source: &'a ArchiveIdentity,
        _destination: &'a ArchiveLocation,
        _snapshot: &'a ArchiveSnapshot,
    ) -> BoxFuture<'a, Result<bool, ImapError>> {
        let call = self.record(Call::Mark(source.clone()));
        (self.mark)(&call)
    }
    fn inspect_deleted<'a>(
        &'a self,
        _source: &'a ArchiveIdentity,
        _destination: &'a ArchiveLocation,
        _snapshot: &'a ArchiveSnapshot,
    ) -> BoxFuture<'a, Result<DeletedSourceState, ImapError>> {
        let call = self.record(Call::InspectDeleted);
        (self.inspect_deleted)(&call)
    }
    fn expunge<'a>(
        &'a self,
        source: &'a ArchiveIdentity,
        destination: &'a ArchiveLocation,
        snapshot: &'a ArchiveSnapshot,
    ) -> BoxFuture<'a, Result<bool, ImapError>> {
        let call = self.record(Call::Expunge(
            source.clone(),
            destination.clone(),
            snapshot.clone(),
        ));
        (self.expunge)(&call)
    }
}

/// A mutable mailbox state shared with reply closures.
fn state<T: Clone + Send + Sync + 'static>(initial: T) -> (Arc<Mutex<T>>, Reply<T>) {
    let cell = Arc::new(Mutex::new(initial));
    let read = cell.clone();
    (
        cell,
        Arc::new(move |_| {
            let value = read.lock().unwrap().clone();
            async move { Ok(value) }.boxed()
        }),
    )
}

struct Env {
    store: TestStore,
    service: ArchiveService,
}

async fn env() -> Env {
    let clock: SharedClock = TestClock::new(NOW);
    let store = TestStore::new(clock.clone()).await;
    let service = ArchiveService::new(store.store.clone(), clock);
    Env { store, service }
}

impl Env {
    fn store(&self) -> &omni_store::Store {
        &self.store.store
    }

    async fn queue(&self, key: &str, identity: ArchiveIdentity) -> ArchiveAction {
        queue_archive_action(self.store(), key, identity)
            .await
            .unwrap()
    }

    async fn update(&self, id: &str, from: S, to: S, patch: ArchivePatch) -> ArchiveAction {
        update_archive_action(self.store(), id, from, to, patch)
            .await
            .unwrap()
    }

    async fn get(&self, id: &str) -> ArchiveAction {
        get_archive_action(self.store(), id).await.unwrap().unwrap()
    }

    async fn echo(&self, message_id: &str, origin: &ArchiveLocation) -> bool {
        is_archive_action_message(self.store(), message_id, Some(origin))
            .await
            .unwrap()
    }
}

fn identity(id: &str) -> ArchiveIdentity {
    ArchiveIdentity {
        folder: "INBOX".to_owned(),
        uid_validity: "10".to_owned(),
        uid: 7,
        message_id: format!("<{id}@example.test>"),
    }
}

fn patch_snapshot(snapshot: ArchiveSnapshot) -> ArchivePatch {
    ArchivePatch {
        snapshot: Some(snapshot),
        ..ArchivePatch::default()
    }
}

fn move_id(call: &Call) -> Option<String> {
    match call {
        Call::Move(s) | Call::Copy(s, _, _) => Some(s.identity.message_id.clone()),
        Call::Mark(i) | Call::Expunge(i, _, _) => Some(i.message_id.clone()),
        _ => None,
    }
}

#[tokio::test]
async fn suppresses_uidplus_echoes_only_at_recorded_coordinates() {
    let e = env().await;
    let source = identity("copy-echo");
    let queued = e.queue("copy-echo", source.clone()).await;
    let archive = loc("Archive", "20", 12);
    let claimed = e
        .update(
            &queued.action_id,
            S::Queued,
            S::CopyClaimed,
            patch_snapshot(uidplus(&[], "Archive")),
        )
        .await;
    assert_eq!(claimed.status, S::CopyClaimed);
    assert!(!e.echo(&source.message_id, &archive).await);
    let verified = e
        .update(
            &queued.action_id,
            S::CopyClaimed,
            S::CopyVerified,
            ArchivePatch {
                destination: Some(archive.clone()),
                ..ArchivePatch::default()
            },
        )
        .await;
    assert_eq!(verified.status, S::CopyVerified);
    assert!(e.echo(&source.message_id, &archive).await);
    assert!(!e.echo(&source.message_id, &loc("Archive", "20", 13)).await);
    assert!(!e.echo(&source.message_id, &source.location()).await);
}

#[tokio::test]
async fn rechecks_a_retained_copy_after_delayed_store_or_expunge_without_mutating() {
    let e = env().await;
    let queued = e.queue("copy-delayed", identity("copy-delayed")).await;
    let destination = loc("Archive", "20", 12);
    let retained = e
        .update(
            &queued.action_id,
            S::Queued,
            S::CopiedSourceRetained,
            ArchivePatch {
                snapshot: Some(uidplus(&[], "Archive")),
                destination: Some(destination.clone()),
                reason: Some(Some(ArchiveReason::CopiedSourceRetained)),
                ..ArchivePatch::default()
            },
        )
        .await;
    let (observed, inspect_deleted) = state(DeletedSourceState::Unmarked);
    let mut mock = transport().with_copy();
    mock.copy = ok(destination.clone());
    mock.inspect_deleted = inspect_deleted;
    let advanced = e
        .service
        .advance_copy(retained, &mock, false, None, false)
        .await
        .unwrap();
    assert_eq!(advanced.status, S::CopiedSourceRetained);
    *observed.lock().unwrap() = DeletedSourceState::Marked;
    e.update(
        &queued.action_id,
        S::CopiedSourceRetained,
        S::CopiedSourceRetained,
        ArchivePatch::next_attempt_at(0),
    )
    .await;
    e.service.sweep(&mock).await.unwrap();
    assert_eq!(
        e.get(&queued.action_id).await.reason,
        Some(ArchiveReason::CopiedSourceDeleted)
    );
    *observed.lock().unwrap() = DeletedSourceState::Absent;
    let latest = e.get(&queued.action_id).await;
    let done = e
        .service
        .advance_copy(latest, &mock, false, None, false)
        .await
        .unwrap();
    assert_eq!(done.status, S::Archived);
    assert_eq!(mock.count_for(&queued.identity.message_id, move_id), 0);
}

#[tokio::test]
async fn rechecks_a_retained_restore_copy_without_deleting_archive_again() {
    let e = env().await;
    let queued = e
        .queue("restore-delayed", identity("restore-delayed"))
        .await;
    let destination = loc("Archive", "20", 12);
    let restored_location = loc("INBOX", "10", 8);
    let retained = e
        .update(
            &queued.action_id,
            S::Queued,
            S::RestoreCopiedSourceRetained,
            ArchivePatch {
                snapshot: Some(uidplus(&[], "Archive")),
                restore_snapshot: Some(uidplus(&["\\Seen"], "INBOX")),
                destination: Some(destination),
                restored_location: Some(restored_location.clone()),
                reason: Some(Some(ArchiveReason::CopiedSourceDeleted)),
                ..ArchivePatch::default()
            },
        )
        .await;
    let mut mock = transport().with_copy();
    mock.copy = ok(restored_location);
    mock.inspect_deleted = ok(DeletedSourceState::Absent);
    let advanced = e
        .service
        .advance_copy(retained, &mock, true, None, false)
        .await
        .unwrap();
    assert_eq!(advanced.status, S::Restored);
    assert_eq!(mock.count(|c| matches!(c, Call::Mark(_))), 0);
    assert_eq!(mock.count(|c| matches!(c, Call::Expunge(..))), 0);
}

#[tokio::test]
async fn does_not_let_retained_receipts_starve_a_newer_queued_action() {
    let e = env().await;
    let destination = loc("Archive", "20", 12);
    for index in 0..21 {
        let key = format!("retained-priority-{index}");
        let queued = e.queue(&key, identity(&key)).await;
        e.update(
            &queued.action_id,
            S::Queued,
            S::CopiedSourceRetained,
            ArchivePatch {
                snapshot: Some(uidplus(&[], "Archive")),
                destination: Some(destination.clone()),
                reason: Some(Some(ArchiveReason::CopiedSourceRetained)),
                ..ArchivePatch::default()
            },
        )
        .await;
    }
    let queued = e.queue("priority-fresh", identity("priority-fresh")).await;
    let mut mock = transport().with_copy();
    mock.copy = ok(destination);
    mock.inspect_deleted = ok(DeletedSourceState::Unmarked);
    e.service.sweep(&mock).await.unwrap();
    assert_eq!(e.get(&queued.action_id).await.status, S::Archived);
    assert_eq!(mock.count(|c| matches!(c, Call::Move(_))), 1);
}

#[tokio::test]
async fn does_not_let_unresolved_copy_claims_starve_a_newer_queued_action() {
    let e = env().await;
    for index in 0..20 {
        let key = format!("uncertain-priority-{index}");
        let queued = e.queue(&key, identity(&key)).await;
        e.update(
            &queued.action_id,
            S::Queued,
            S::CopyClaimed,
            ArchivePatch {
                snapshot: Some(uidplus(&[], "Archive")),
                reason: Some(Some(ArchiveReason::CopyUncertain)),
                ..ArchivePatch::default()
            },
        )
        .await;
    }
    let queued = e
        .queue("priority-after-claims", identity("priority-after-claims"))
        .await;
    let mut mock = transport().with_copy();
    mock.copy = ok(loc("Archive", "20", 12));
    e.service.sweep(&mock).await.unwrap();
    assert_eq!(e.get(&queued.action_id).await.status, S::Archived);
    assert_eq!(mock.count(|c| matches!(c, Call::Move(_))), 1);
}

#[tokio::test]
async fn persists_copy_deleted_and_uid_expunge_claims_before_each_mutation() {
    let e = env().await;
    let queued = e.queue("copy-stages", identity("copy-stages")).await;
    let destination = loc("Archive", "20", 12);
    let store = e.store().clone();
    let id = queued.action_id.clone();
    let source_state = Arc::new(Mutex::new(DeletedSourceState::Unmarked));
    let status_during = |expected: S| -> Reply<bool> {
        let (store, id) = (store.clone(), id.clone());
        let source_state = source_state.clone();
        Arc::new(move |_| {
            let (store, id, source_state) = (store.clone(), id.clone(), source_state.clone());
            async move {
                assert_eq!(
                    get_archive_action(&store, &id)
                        .await
                        .unwrap()
                        .unwrap()
                        .status,
                    expected
                );
                *source_state.lock().unwrap() = if expected == S::DeleteClaimed {
                    DeletedSourceState::Marked
                } else {
                    DeletedSourceState::Absent
                };
                Ok(true)
            }
            .boxed()
        })
    };
    let mut mock = transport().with_copy();
    mock.inspect = ok(uidplus(&["\\Flagged"], "Archive"));
    mock.copy = {
        let (store, id, destination) = (store.clone(), id.clone(), destination.clone());
        Arc::new(move |_| {
            let (store, id, destination) = (store.clone(), id.clone(), destination.clone());
            async move {
                assert_eq!(
                    get_archive_action(&store, &id)
                        .await
                        .unwrap()
                        .unwrap()
                        .status,
                    S::CopyClaimed
                );
                Ok(destination)
            }
            .boxed()
        })
    };
    mock.reconcile_copy = ok(moved(destination.clone(), &["\\Flagged"]));
    mock.mark = status_during(S::DeleteClaimed);
    mock.expunge = status_during(S::ExpungeClaimed);
    let reader = source_state.clone();
    mock.inspect_deleted = Arc::new(move |_| {
        let value = *reader.lock().unwrap();
        async move { Ok(value) }.boxed()
    });
    let result = e.service.process(queued, &mock).await.unwrap();
    assert_eq!(result.status, S::Archived);
    assert_eq!(result.destination, Some(destination));
    assert_eq!(mock.count(|c| matches!(c, Call::Copy(..))), 1);
    assert_eq!(mock.count(|c| matches!(c, Call::Mark(_))), 1);
    assert_eq!(mock.count(|c| matches!(c, Call::Expunge(..))), 1);
}

#[tokio::test]
async fn never_recopies_after_a_lost_copy_response_and_keeps_status_reconciliation_read_only() {
    let e = env().await;
    let queued = e.queue("copy-lost", identity("copy-lost")).await;
    let destination = loc("Archive", "20", 12);
    let (copied, reconcile_copy) = state(ArchiveReconcileResult::Uncertain);
    let mut mock = transport().with_copy();
    mock.inspect = ok(uidplus(&[], "Archive"));
    mock.copy = fail("response lost");
    mock.reconcile_copy = reconcile_copy;
    mock.inspect_deleted = ok(DeletedSourceState::Unmarked);
    let first = e.service.process(queued.clone(), &mock).await.unwrap();
    assert_eq!(first.status, S::CopyClaimed);
    *copied.lock().unwrap() = moved(destination, &[]);
    let reconciled = e
        .service
        .advance_copy(first, &mock, false, None, false)
        .await
        .unwrap();
    assert_eq!(reconciled.status, S::CopyVerified);
    assert_eq!(mock.count(|c| matches!(c, Call::Mark(_))), 0);
    e.service.sweep(&mock).await.unwrap();
    assert_eq!(mock.count(|c| matches!(c, Call::Copy(..))), 1);
    assert_eq!(mock.count(|c| matches!(c, Call::Mark(_))), 1);
    assert_eq!(mock.count(|c| matches!(c, Call::Expunge(..))), 0);
}

#[tokio::test]
async fn persists_copyuid_before_reconciliation_and_rejects_a_different_uid_after_restart() {
    let e = env().await;
    let queued = e
        .queue("copyuid-restart", identity("copyuid-restart"))
        .await;
    let claimed = e
        .update(
            &queued.action_id,
            S::Queued,
            S::CopyClaimed,
            patch_snapshot(uidplus(&[], "Archive")),
        )
        .await;
    let mapped = loc("Archive", "20", 12);
    let mut mock = transport().with_copy();
    mock.copy = ok(mapped.clone());
    mock.reconcile_copy = ok(moved(loc("Archive", "20", 13), &[]));
    mock.inspect_deleted = ok(DeletedSourceState::Unmarked);
    let first = e
        .service
        .advance_copy(claimed, &mock, false, Some(mapped.clone()), true)
        .await
        .unwrap();
    assert_eq!(first.status, S::CopyClaimed);
    assert_eq!(first.destination, Some(mapped.clone()));
    let restarted = e.get(&queued.action_id).await;
    let second = e
        .service
        .advance_copy(restarted, &mock, false, None, true)
        .await
        .unwrap();
    assert_eq!(second.status, S::CopyClaimed);
    assert_eq!(second.destination, Some(mapped));
    assert_eq!(
        mock.count(|c| matches!(c, Call::Copy(..) | Call::Mark(_) | Call::Expunge(..))),
        0
    );
}

#[tokio::test]
async fn holds_status_reconciliation_until_an_in_flight_copy_records_its_uid() {
    let e = env().await;
    let queued = e
        .queue("copy-status-concurrent", identity("copy-status-concurrent"))
        .await;
    let destination = loc("Archive", "20", 12);
    let copy_started = Arc::new(Notify::new());
    let release_copy = Arc::new(Notify::new());
    let (source_state, inspect_deleted) = state(DeletedSourceState::Marked);
    let mut mock = transport().with_copy();
    mock.inspect = ok(uidplus(&[], "Archive"));
    mock.copy = {
        let (started, release, destination) = (
            copy_started.clone(),
            release_copy.clone(),
            destination.clone(),
        );
        Arc::new(move |_| {
            let (started, release, destination) =
                (started.clone(), release.clone(), destination.clone());
            async move {
                started.notify_one();
                release.notified().await;
                Ok(destination)
            }
            .boxed()
        })
    };
    mock.reconcile_copy = ok(moved(destination.clone(), &[]));
    mock.inspect_deleted = inspect_deleted;
    let expunged = source_state.clone();
    mock.expunge = Arc::new(move |_| {
        *expunged.lock().unwrap() = DeletedSourceState::Absent;
        async { Ok(true) }.boxed()
    });
    let worker = {
        let (service, mock) = (e.service.clone(), mock.clone());
        tokio::spawn(async move { service.sweep(&mock).await })
    };
    copy_started.notified().await;
    let status = {
        let (service, mock, id) = (e.service.clone(), mock.clone(), queued.action_id.clone());
        tokio::spawn(async move { service.status(&id, Some(&mock)).await })
    };
    tokio::task::yield_now().await;
    assert_eq!(e.get(&queued.action_id).await.status, S::CopyClaimed);
    assert_eq!(mock.count(|c| matches!(c, Call::ReconcileCopy)), 0);
    release_copy.notify_one();
    worker.await.unwrap().unwrap();
    assert_eq!(status.await.unwrap().unwrap().status, S::Archived);
    assert_eq!(
        e.get(&queued.action_id).await.destination,
        Some(destination)
    );
}

#[tokio::test]
async fn restores_only_the_recorded_archive_uid_through_the_same_scoped_stages() {
    let e = env().await;
    let queued = e.queue("copy-restore", identity("copy-restore")).await;
    let destination = loc("Archive", "20", 12);
    let archived = e
        .update(
            &queued.action_id,
            S::Queued,
            S::Archived,
            ArchivePatch {
                snapshot: Some(uidplus(&["\\Flagged"], "Archive")),
                destination: Some(destination.clone()),
                ..ArchivePatch::default()
            },
        )
        .await;
    assert_eq!(archived.status, S::Archived);
    let (source_state, inspect_deleted) = state(DeletedSourceState::Marked);
    let mut mock = transport().with_copy();
    mock.copy = ok(loc("INBOX", "10", 8));
    mock.reconcile_copy = ok(moved(loc("INBOX", "10", 8), &["\\Seen"]));
    mock.inspect_deleted = inspect_deleted;
    let expunged = source_state.clone();
    mock.expunge = Arc::new(move |_| {
        *expunged.lock().unwrap() = DeletedSourceState::Absent;
        async { Ok(true) }.boxed()
    });
    let restored = e.service.restore(&queued.action_id, &mock).await.unwrap();
    assert_eq!(restored.status, S::Restored);
    assert_eq!(restored.restored_location, Some(loc("INBOX", "10", 8)));
    let source = destination.with_message_id(&queued.identity.message_id);
    let copy = mock.calls().into_iter().find_map(|c| match c {
        Call::Copy(s, target, snap) => Some((s, target, snap)),
        _ => None,
    });
    let (copy_source, target, snap) = copy.unwrap();
    assert_eq!(copy_source.identity, source);
    assert_eq!(target, "INBOX");
    assert_eq!(snap.flags, vec!["\\Seen"]);
    let expunge = mock.calls().into_iter().find_map(|c| match c {
        Call::Expunge(s, d, snap) => Some((s, d, snap)),
        _ => None,
    });
    let (expunge_source, expunge_destination, snap) = expunge.unwrap();
    assert_eq!(expunge_source, source);
    assert_eq!(Some(expunge_destination), restored.restored_location);
    assert_eq!(snap.flags, vec!["\\Seen"]);
}

#[tokio::test]
async fn reconciles_claimed_store_and_expunge_on_status_without_mutating() {
    let e = env().await;
    let queued = e.queue("copy-status", identity("copy-status")).await;
    let destination = loc("Archive", "20", 12);
    let claimed = e
        .update(
            &queued.action_id,
            S::Queued,
            S::DeleteClaimed,
            ArchivePatch {
                snapshot: Some(uidplus(&["\\Flagged"], "Archive")),
                destination: Some(destination.clone()),
                ..ArchivePatch::default()
            },
        )
        .await;
    let (source_state, inspect_deleted) = state(DeletedSourceState::Marked);
    let mut mock = transport().with_copy();
    mock.copy = ok(destination);
    mock.inspect_deleted = inspect_deleted;
    let advanced = e
        .service
        .advance_copy(claimed, &mock, false, None, false)
        .await
        .unwrap();
    assert_eq!(advanced.status, S::DeleteClaimed);
    assert_eq!(mock.count(|c| matches!(c, Call::Expunge(..))), 0);
    let expunge_claimed = e
        .update(
            &queued.action_id,
            S::DeleteClaimed,
            S::ExpungeClaimed,
            ArchivePatch::default(),
        )
        .await;
    *source_state.lock().unwrap() = DeletedSourceState::Absent;
    let done = e
        .service
        .advance_copy(expunge_claimed, &mock, false, None, false)
        .await
        .unwrap();
    assert_eq!(done.status, S::Archived);
    assert_eq!(
        mock.count(|c| matches!(c, Call::Mark(_) | Call::Expunge(..))),
        0
    );
}

#[tokio::test]
async fn reserves_exact_message_id_rejects_a_conflicting_key_and_cancels_before_move() {
    let e = env().await;
    let original = identity("queue");
    let first = e.queue("queue-key", original.clone()).await;
    assert_eq!(first.status, S::Queued);
    assert!(!e.echo(&original.message_id, &original.location()).await);
    assert_eq!(e.queue("queue-key", original.clone()).await, first);
    let error = queue_archive_action(e.store(), "queue-key", identity("other"))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("different message"), "{error}");
    let error = queue_archive_action(e.store(), "another-key", original.clone())
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("already has an archive action"),
        "{error}"
    );
    e.update(
        &first.action_id,
        S::Queued,
        S::Cancelled,
        ArchivePatch::default(),
    )
    .await;
    assert_eq!(e.get(&first.action_id).await.status, S::Cancelled);
    let replacement = e.queue("replacement-key", original).await;
    assert_eq!(replacement.status, S::Queued);
}

#[tokio::test]
async fn records_the_claim_before_move_and_never_repeats_an_uncertain_move() {
    let e = env().await;
    let queued = e.queue("uncertain-key", identity("uncertain")).await;
    let mut mock = transport();
    mock.move_message = fail("connection lost");
    mock.reconcile = ok(ArchiveReconcileResult::Uncertain);
    let first = e.service.process(queued.clone(), &mock).await.unwrap();
    assert_eq!(first.status, S::Uncertain);
    assert_eq!(first.snapshot.unwrap().source_hash, "abc");
    assert_eq!(mock.count(|c| matches!(c, Call::Move(_))), 1);
    e.service.sweep(&mock).await.unwrap();
    assert_eq!(mock.count_for(&queued.identity.message_id, move_id), 1);
}

#[tokio::test]
async fn keeps_a_cancelled_action_out_of_the_worker_sweep() {
    let e = env().await;
    let queued = e.queue("cancel-sweep", identity("cancel-sweep")).await;
    e.update(
        &queued.action_id,
        S::Queued,
        S::Cancelled,
        ArchivePatch::default(),
    )
    .await;
    let mock = transport();
    e.service.sweep(&mock).await.unwrap();
    assert_eq!(mock.count(|c| matches!(c, Call::Move(_))), 0);
}

#[tokio::test]
async fn suppresses_only_action_created_archive_and_restored_inbox_origins() {
    let e = env().await;
    let source = identity("suppression");
    let queued = e.queue("suppression-key", source.clone()).await;
    let archive = loc("Archive", "20", 12);
    let restored = loc("INBOX", "10", 8);
    assert!(!e.echo(&source.message_id, &archive).await);
    e.update(
        &queued.action_id,
        S::Queued,
        S::Claimed,
        patch_snapshot(snapshot(&[])),
    )
    .await;
    assert!(!e.echo(&source.message_id, &source.location()).await);
    assert!(e.echo(&source.message_id, &archive).await);
    e.update(
        &queued.action_id,
        S::Claimed,
        S::Archived,
        ArchivePatch {
            destination: Some(archive.clone()),
            ..ArchivePatch::default()
        },
    )
    .await;
    assert!(e.echo(&source.message_id, &archive).await);
    assert!(!e.echo(&source.message_id, &loc("Archive", "20", 13)).await);
    assert!(!e.echo(&source.message_id, &source.location()).await);
    e.update(
        &queued.action_id,
        S::Archived,
        S::RestoreClaimed,
        ArchivePatch::default(),
    )
    .await;
    assert!(e.echo(&source.message_id, &restored).await);
    e.update(
        &queued.action_id,
        S::RestoreClaimed,
        S::Restored,
        ArchivePatch {
            restored_location: Some(restored.clone()),
            ..ArchivePatch::default()
        },
    )
    .await;
    assert!(e.echo(&source.message_id, &restored).await);
    assert!(!e.echo(&source.message_id, &source.location()).await);
}

#[tokio::test]
async fn keeps_a_delayed_restore_event_suppressed_after_immediate_requeue() {
    let e = env().await;
    let initial = identity("requeue-history");
    let first = e.queue("requeue-history-first", initial.clone()).await;
    let archived = loc("Archive", "20", 42);
    let restored = loc("INBOX", "10", 43);
    e.update(
        &first.action_id,
        S::Queued,
        S::Claimed,
        patch_snapshot(snapshot(&[])),
    )
    .await;
    e.update(
        &first.action_id,
        S::Claimed,
        S::Archived,
        ArchivePatch {
            destination: Some(archived.clone()),
            ..ArchivePatch::default()
        },
    )
    .await;
    e.update(
        &first.action_id,
        S::Archived,
        S::RestoreClaimed,
        ArchivePatch::default(),
    )
    .await;
    e.update(
        &first.action_id,
        S::RestoreClaimed,
        S::Restored,
        ArchivePatch {
            restored_location: Some(restored.clone()),
            ..ArchivePatch::default()
        },
    )
    .await;
    let second = e
        .queue(
            "requeue-history-second",
            restored.with_message_id(&initial.message_id),
        )
        .await;
    assert_eq!(second.status, S::Queued);
    assert!(!e.echo(&initial.message_id, &initial.location()).await);
    assert!(e.echo(&initial.message_id, &restored).await);
    assert!(e.echo(&initial.message_id, &archived).await);
}

#[tokio::test]
async fn uses_the_latest_reservation_marker_when_an_older_row_has_no_history_key() {
    let e = env().await;
    let source = identity("legacy-marker");
    let action = e.queue("legacy-marker-key", source.clone()).await;
    let archived = loc("Archive", "20", 44);
    e.update(
        &action.action_id,
        S::Queued,
        S::Claimed,
        patch_snapshot(snapshot(&[])),
    )
    .await;
    e.update(
        &action.action_id,
        S::Claimed,
        S::Archived,
        ArchivePatch {
            destination: Some(archived.clone()),
            ..ArchivePatch::default()
        },
    )
    .await;
    delete_history(&e, &source.message_id).await;
    assert!(e.echo(&source.message_id, &archived).await);
}

async fn delete_history(e: &Env, message_id: &str) {
    let pk = history_key_for(message_id);
    e.store().write(move |tx| tx.delete_doc(&pk)).await.unwrap();
}

#[tokio::test]
async fn protects_exact_archived_uid_and_skips_ambiguous_archive_auto_read() {
    let e = env().await;
    let source = identity("auto-read-protection");
    let queued = e.queue("auto-read-protection", source.clone()).await;
    assert!(
        !archive_auto_read_protection(e.store(), "Archive", Some("20"))
            .await
            .unwrap()
            .skip
    );
    e.update(
        &queued.action_id,
        S::Queued,
        S::Claimed,
        patch_snapshot(snapshot(&[])),
    )
    .await;
    let uncertain = archive_auto_read_protection(e.store(), "Archive", Some("20"))
        .await
        .unwrap();
    assert!(!uncertain.skip);
    assert!(uncertain.fallback_message_ids.contains(&source.message_id));
    e.update(
        &queued.action_id,
        S::Claimed,
        S::Archived,
        ArchivePatch {
            destination: Some(loc("Archive", "20", 12)),
            ..ArchivePatch::default()
        },
    )
    .await;
    let protected = archive_auto_read_protection(e.store(), "Archive", Some("20"))
        .await
        .unwrap();
    assert!(!protected.skip);
    assert!(protected.excluded_uids.contains(&12));
    let stale = archive_auto_read_protection(e.store(), "Archive", Some("21"))
        .await
        .unwrap();
    assert!(!stale.skip);
    assert!(stale.fallback_message_ids.contains(&source.message_id));
}

#[tokio::test]
async fn fails_a_missing_native_move_capability_without_trying_a_mutation() {
    let e = env().await;
    let queued = e.queue("no-move", identity("no-move")).await;
    let mut mock = transport();
    mock.inspect = fail("Server does not advertise MOVE");
    let result = e.service.process(queued, &mock).await.unwrap();
    assert_eq!(result.status, S::Failed);
    assert_eq!(result.reason, Some(ArchiveReason::NativeMoveUnavailable));
    assert_eq!(result.attempts, 1);
    assert_eq!(mock.count(|c| matches!(c, Call::Move(_))), 0);
}

#[tokio::test]
async fn reconciles_a_claimed_action_after_restart_without_repeating_move() {
    let e = env().await;
    let queued = e
        .queue("claimed-restart", identity("claimed-restart"))
        .await;
    e.update(
        &queued.action_id,
        S::Queued,
        S::Claimed,
        patch_snapshot(snapshot(&["\\Flagged"])),
    )
    .await;
    let mock = transport();
    e.service.sweep(&mock).await.unwrap();
    assert_eq!(mock.count(|c| matches!(c, Call::Move(_))), 0);
    assert_eq!(e.get(&queued.action_id).await.status, S::Archived);
}

#[tokio::test]
async fn archives_with_verified_receipt_and_restores_with_current_archive_flags() {
    let e = env().await;
    let queued = e.queue("restore-key", identity("restore")).await;
    let mock = transport();
    let archived = e.service.process(queued.clone(), &mock).await.unwrap();
    assert_eq!(archived.status, S::Archived);
    let restored = e.service.restore(&queued.action_id, &mock).await.unwrap();
    assert_eq!(restored.status, S::Restored);
    let last_verify = mock.calls().into_iter().rev().find_map(|c| match c {
        Call::Verify(location, id, hash, flags) => Some((location, id, hash, flags)),
        _ => None,
    });
    assert_eq!(
        last_verify,
        Some((
            loc("INBOX", "10", 8),
            identity("restore").message_id,
            "abc".to_owned(),
            vec!["\\Seen".to_owned()]
        ))
    );
}

#[tokio::test]
async fn leaves_restore_uncertain_when_the_archive_source_is_not_confirmed_gone() {
    let e = env().await;
    let queued = e
        .queue("restore-uncertain", identity("restore-uncertain"))
        .await;
    let mut mock = transport();
    mock.reconcile_restore = ok(ArchiveReconcileResult::Uncertain);
    e.service.process(queued.clone(), &mock).await.unwrap();
    let restored = e.service.restore(&queued.action_id, &mock).await.unwrap();
    assert_eq!(restored.status, S::RestoreUncertain);
    assert_eq!(mock.count(|c| matches!(c, Call::Restore)), 1);
}

// ---- archive actions for copies that share a Message-ID -----------------

fn inbox(message_id: &str, uid: u32, uid_validity: &str) -> ArchiveIdentity {
    ArchiveIdentity {
        folder: "INBOX".to_owned(),
        uid_validity: uid_validity.to_owned(),
        uid,
        message_id: message_id.to_owned(),
    }
}

fn archive_uid(uid: u32) -> u32 {
    52802 + (604 - uid)
}

fn source_uid(call: &Call) -> u32 {
    match call {
        Call::Move(s) | Call::Reconcile(s, _) => s.identity.uid,
        _ => 0,
    }
}

fn per_copy(reconcile_uncertain: bool) -> Mock {
    let mut mock = transport();
    mock.move_message = Arc::new(|call| {
        let uid = archive_uid(source_uid(call));
        async move {
            Ok(ArchiveMoveResult {
                destination: loc("Archive", "20", uid),
                snapshot: snapshot(&["\\Flagged"]),
            })
        }
        .boxed()
    });
    mock.reconcile = Arc::new(move |call| {
        let uid = archive_uid(source_uid(call));
        async move {
            Ok(if reconcile_uncertain {
                ArchiveReconcileResult::Uncertain
            } else {
                moved(loc("Archive", "20", uid), &["\\Flagged"])
            })
        }
        .boxed()
    });
    mock
}

const UV: &str = "1318024686";

#[tokio::test]
async fn archives_a_second_physical_copy_after_the_first_copy_is_archived() {
    let e = env().await;
    let message_id = "<B7tN/sj-RWeYS-AAAAALBnCQCEcAMATfMrPdIT-customerservice@novusnow.ca>";
    let mock = per_copy(false);
    let first = e.queue("novus-604", inbox(message_id, 604, UV)).await;
    let error = queue_archive_action(e.store(), "novus-603-early", inbox(message_id, 603, UV))
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("Another copy") && error.to_string().contains("unresolved"),
        "{error}"
    );
    let archived = e.service.process(first.clone(), &mock).await.unwrap();
    assert_eq!(archived.status, S::Archived);
    assert_eq!(archived.destination, Some(loc("Archive", "20", 52802)));
    let second = e.queue("novus-603", inbox(message_id, 603, UV)).await;
    assert_eq!(second.status, S::Queued);
    assert_ne!(second.action_id, first.action_id);
    assert_eq!(
        e.service.process(second, &mock).await.unwrap().status,
        S::Archived
    );
    let claimed = vec![loc("Archive", "20", 52802)];
    let last_inspect = mock.calls().into_iter().rev().find_map(|c| match c {
        Call::Inspect(s) => Some(s),
        _ => None,
    });
    let last_inspect = last_inspect.unwrap();
    assert_eq!(
        (
            last_inspect.identity.uid,
            last_inspect.claimed_copies.clone()
        ),
        (603, claimed.clone())
    );
    let last_reconcile = mock.calls().into_iter().rev().find_map(|c| match c {
        Call::Reconcile(s, hash) => Some((s, hash)),
        _ => None,
    });
    let (request, hash) = last_reconcile.unwrap();
    assert_eq!(
        (request.identity.uid, request.claimed_copies, hash.as_str()),
        (603, claimed, "abc")
    );
    let first_now = e.get(&first.action_id).await;
    assert_eq!(first_now.status, S::Archived);
    assert_eq!(first_now.identity, inbox(message_id, 604, UV));
    assert_eq!(first_now.destination.map(|d| d.uid), Some(52802));
}

#[tokio::test]
async fn returns_the_completed_receipt_for_a_retry_and_refuses_a_new_key_for_that_copy() {
    let e = env().await;
    let source = inbox("<retry-copy@example.test>", 604, UV);
    let queued = e.queue("retry-copy", source.clone()).await;
    let archived = e.service.process(queued, &per_copy(false)).await.unwrap();
    assert_eq!(archived.status, S::Archived);
    assert_eq!(e.queue("retry-copy", source.clone()).await, archived);
    let error = queue_archive_action(e.store(), "retry-copy-other-key", source)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("already has an archive action"),
        "{error}"
    );
}

#[tokio::test]
async fn blocks_sibling_copies_and_restores_while_a_move_outcome_is_unproven() {
    let e = env().await;
    let message_id = "<uncertain-copy@example.test>";
    let settled = e
        .queue("uncertain-copy-602", inbox(message_id, 602, UV))
        .await;
    e.service
        .process(settled.clone(), &per_copy(false))
        .await
        .unwrap();
    let queued = e
        .queue("uncertain-copy-604", inbox(message_id, 604, UV))
        .await;
    let mut lost = transport();
    lost.move_message = fail("connection lost");
    lost.reconcile = ok(ArchiveReconcileResult::Uncertain);
    assert_eq!(
        e.service
            .process(queued.clone(), &lost)
            .await
            .unwrap()
            .status,
        S::Uncertain
    );
    let error = queue_archive_action(e.store(), "uncertain-copy-603", inbox(message_id, 603, UV))
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("unresolved archive action"),
        "{error}"
    );
    let error = e
        .service
        .restore(&settled.action_id, &per_copy(false))
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("unresolved archive action"),
        "{error}"
    );
    assert_eq!(lost.count(|c| matches!(c, Call::Restore)), 0);
    e.update(
        &queued.action_id,
        S::Uncertain,
        S::Uncertain,
        ArchivePatch::next_attempt_at(0),
    )
    .await;
    e.service.sweep(&per_copy(false)).await.unwrap();
    assert_eq!(e.get(&queued.action_id).await.status, S::Archived);
    assert_eq!(lost.count(|c| matches!(c, Call::Move(_))), 1);
    assert_eq!(
        e.queue("uncertain-copy-603", inbox(message_id, 603, UV))
            .await
            .status,
        S::Queued
    );
}

#[tokio::test]
async fn treats_a_uid_under_a_new_uidvalidity_as_a_different_source_copy() {
    let e = env().await;
    let message_id = "<validity-copy@example.test>";
    let old = e.queue("validity-old", inbox(message_id, 604, "1")).await;
    e.update(
        &old.action_id,
        S::Queued,
        S::Claimed,
        ArchivePatch::default(),
    )
    .await;
    let error = queue_archive_action(e.store(), "validity-new", inbox(message_id, 604, "2"))
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("unresolved archive action"),
        "{error}"
    );
    e.update(
        &old.action_id,
        S::Claimed,
        S::Failed,
        ArchivePatch::reason(ArchiveReason::SourceUnavailable),
    )
    .await;
    let renumbered = e.queue("validity-new", inbox(message_id, 604, "2")).await;
    assert_eq!(renumbered.identity.uid_validity, "2");
    let error = queue_archive_action(e.store(), "validity-old", inbox(message_id, 604, "2"))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("different message"), "{error}");
}

#[tokio::test]
async fn reads_legacy_marker_only_reservations_per_source_copy() {
    let e = env().await;
    let message_id = "<legacy-copy@example.test>";
    let legacy = e.queue("legacy-copy-604", inbox(message_id, 604, UV)).await;
    delete_history(&e, message_id).await;
    let error = queue_archive_action(e.store(), "legacy-copy-603", inbox(message_id, 603, UV))
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("unresolved archive action"),
        "{error}"
    );
    e.service.process(legacy, &per_copy(false)).await.unwrap();
    let sibling = e.queue("legacy-copy-603", inbox(message_id, 603, UV)).await;
    assert_eq!(sibling.status, S::Queued);
    let error = queue_archive_action(
        e.store(),
        "legacy-copy-604-again",
        inbox(message_id, 604, UV),
    )
    .await
    .unwrap_err();
    assert!(
        error.to_string().contains("already has an archive action"),
        "{error}"
    );
    assert!(e.echo(message_id, &loc("Archive", "20", 52802)).await);
}

#[tokio::test]
async fn does_not_suppress_a_pre_existing_inbox_sibling_during_a_restore() {
    let e = env().await;
    let message_id = "<restore-sibling@example.test>";
    let queued = e.queue("restore-sibling", inbox(message_id, 604, UV)).await;
    e.service
        .process(queued.clone(), &per_copy(false))
        .await
        .unwrap();
    e.update(
        &queued.action_id,
        S::Archived,
        S::RestoreClaimed,
        ArchivePatch::default(),
    )
    .await;
    assert!(
        !e.echo(message_id, &inbox(message_id, 603, UV).location())
            .await
    );
    assert!(
        e.echo(message_id, &inbox(message_id, 900, UV).location())
            .await
    );
}

#[tokio::test]
async fn suppresses_only_restore_echoes_above_every_known_inbox_copy() {
    let e = env().await;
    let message_id = "<restore-window@example.test>";
    let first = e
        .queue("restore-window-604", inbox(message_id, 604, UV))
        .await;
    e.service
        .process(first.clone(), &per_copy(false))
        .await
        .unwrap();
    let later = e
        .queue("restore-window-700", inbox(message_id, 700, UV))
        .await;
    e.update(
        &later.action_id,
        S::Queued,
        S::Cancelled,
        ArchivePatch::default(),
    )
    .await;
    e.update(
        &first.action_id,
        S::Archived,
        S::RestoreClaimed,
        ArchivePatch::default(),
    )
    .await;
    let event = |uid| inbox(message_id, uid, UV).location();
    assert!(!e.echo(message_id, &event(650)).await);
    assert!(!e.echo(message_id, &event(700)).await);
    assert!(e.echo(message_id, &event(701)).await);
    e.update(
        &first.action_id,
        S::RestoreClaimed,
        S::RestoreUncertain,
        ArchivePatch {
            restored_location: Some(loc("INBOX", UV, 800)),
            ..ArchivePatch::default()
        },
    )
    .await;
    assert!(!e.echo(message_id, &event(750)).await);
    assert!(!e.echo(message_id, &event(900)).await);
    assert!(e.echo(message_id, &event(800)).await);
}

#[tokio::test]
async fn defers_a_queued_copy_while_a_sibling_restore_is_unproven() {
    let e = env().await;
    let message_id = "<toctou-copy@example.test>";
    let first = e.queue("toctou-604", inbox(message_id, 604, UV)).await;
    e.service
        .process(first.clone(), &per_copy(false))
        .await
        .unwrap();
    let second = e.queue("toctou-603", inbox(message_id, 603, UV)).await;
    e.update(
        &first.action_id,
        S::Archived,
        S::RestoreClaimed,
        ArchivePatch::default(),
    )
    .await;
    e.update(
        &first.action_id,
        S::RestoreClaimed,
        S::RestoreUncertain,
        ArchivePatch::default(),
    )
    .await;
    let mock = per_copy(false);
    let deferred = e.service.process(second.clone(), &mock).await.unwrap();
    assert_eq!((deferred.status, deferred.attempts), (S::Queued, 0));
    assert!(deferred.next_attempt_at > second.next_attempt_at);
    assert_eq!(
        mock.count(|c| matches!(c, Call::Inspect(_) | Call::Move(_))),
        0
    );
    e.update(
        &first.action_id,
        S::RestoreUncertain,
        S::Restored,
        ArchivePatch {
            restored_location: Some(loc("INBOX", UV, 900)),
            ..ArchivePatch::default()
        },
    )
    .await;
    assert_eq!(
        e.service.process(deferred, &mock).await.unwrap().status,
        S::Archived
    );
}
