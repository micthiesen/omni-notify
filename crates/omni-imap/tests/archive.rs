//! Exact archive moves: native MOVE, verified UIDPLUS fallback,
//! identical-bytes duplicate deliveries.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Mutex;

use omni_imap::fake::{FakeCall, FakeMessage, FakeOp, FakeServer};
use omni_imap::ops::archive::{
    ArchiveIdentity, ArchiveLocation, ArchiveReconcileResult, ArchiveSourceRequest,
    ArchiveStrategy, DeletedSourceState, copy_exact_archive_message, expunge_exact_archive_source,
    inspect_archive_source, inspect_exact_deleted_source, mark_exact_archive_source_deleted,
    move_archive_message, reconcile_archive_message, reconcile_exact_copy, restore_archive_message,
    verify_archive_location,
};
use omni_imap::protocol::{CopyUid, ImapError};

const ARCHIVE: &str = "iCloud Archive";

fn identity() -> ArchiveIdentity {
    ArchiveIdentity {
        folder: "INBOX".to_owned(),
        uid_validity: "10".to_owned(),
        uid: 7,
        message_id: "<one@example.test>".to_owned(),
    }
}

fn request(identity: ArchiveIdentity) -> ArchiveSourceRequest {
    ArchiveSourceRequest::from(identity)
}

fn message() -> FakeMessage {
    FakeMessage::new(b"Message-ID: <one@example.test>\r\n\r\nprivate body".to_vec())
        .flags(&["\\Seen", "\\Flagged"])
}

fn fixture(move_supported: bool, uid_plus: bool) -> FakeServer {
    let mut caps = vec!["IMAP4REV1"];
    if move_supported {
        caps.push("MOVE");
    }
    if uid_plus {
        caps.push("UIDPLUS");
    }
    let server = FakeServer::new(&caps);
    server
        .folder("INBOX", 10, None)
        .folder(ARCHIVE, 20, Some("\\Archive"));
    server.put("INBOX", 7, message());
    // New Archive UIDs start at 12 (Inbox continues at 8).
    server.lock().folder_mut(ARCHIVE).unwrap().uid_next = 12;
    server
}

fn moves(server: &FakeServer) -> usize {
    server.count(|c| matches!(c, FakeCall::Move { .. }))
}

fn stores(server: &FakeServer) -> usize {
    server.count(|c| matches!(c, FakeCall::Store { .. }))
}

fn location(folder: &str, uv: &str, uid: u32) -> ArchiveLocation {
    ArchiveLocation {
        folder: folder.to_owned(),
        uid_validity: uv.to_owned(),
        uid,
    }
}

#[tokio::test]
async fn refuses_a_server_without_move_before_any_mailbox_mutation() {
    let server = fixture(false, false);
    let mut client = server.client();
    let error = inspect_archive_source(&mut client, &request(identity()))
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("neither MOVE nor UIDPLUS"),
        "{error}"
    );
    let error = move_archive_message(&mut client, &request(identity()), "unknown")
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("neither MOVE nor UIDPLUS"),
        "{error}"
    );
    assert_eq!(moves(&server), 0);
}

#[tokio::test]
async fn requires_the_exact_inbox_uidvalidity_and_message_id() {
    let server = fixture(true, false);
    let mut client = server.client();
    let changed = ArchiveIdentity {
        uid_validity: "11".to_owned(),
        ..identity()
    };
    let error = inspect_archive_source(&mut client, &request(changed))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("UIDVALIDITY changed"), "{error}");
    let other = ArchiveIdentity {
        message_id: "<other@example.test>".to_owned(),
        ..identity()
    };
    let error = inspect_archive_source(&mut client, &request(other))
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("different Message-ID"),
        "{error}"
    );
    assert_eq!(moves(&server), 0);
}

#[tokio::test]
async fn rejects_a_changed_mime_source_and_an_existing_identical_archive_copy() {
    let server = fixture(true, false);
    let mut client = server.client();
    let error = move_archive_message(&mut client, &request(identity()), "wrong-hash")
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Source changed since reservation"),
        "{error}"
    );
    server.put(ARCHIVE, 11, message());
    let error = inspect_archive_source(&mut client, &request(identity()))
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Matching Archive copy already exists"),
        "{error}"
    );
    let error = move_archive_message(&mut client, &request(identity()), "wrong-hash")
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Matching Archive copy already exists"),
        "{error}"
    );
    assert_eq!(moves(&server), 0);
}

#[tokio::test]
async fn refuses_an_unconfirmed_archive_duplicate_search() {
    let server = fixture(true, false);
    server.lock().search_override = Some(Box::new(|_, _| Some(Ok(None))));
    let mut client = server.client();
    let error = inspect_archive_source(&mut client, &request(identity()))
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("search was not confirmed"),
        "{error}"
    );
    assert_eq!(moves(&server), 0);
}

#[tokio::test]
async fn treats_failed_source_or_candidate_reads_as_uncertain() {
    let server = fixture(true, false);
    let mut client = server.client();
    let snapshot = inspect_archive_source(&mut client, &request(identity()))
        .await
        .unwrap();
    server.put(ARCHIVE, 12, message());
    server.lock().fetch_override = Some(Box::new(|folder, _, _| {
        (folder == "INBOX").then(|| Err(ImapError::new("UID FETCH", "source read failed")))
    }));
    let result =
        reconcile_archive_message(&mut client, &request(identity()), &snapshot.source_hash)
            .await
            .unwrap();
    assert_eq!(result, ArchiveReconcileResult::Uncertain);
    server.lock().fetch_override = Some(Box::new(|folder, _, _| {
        (folder == ARCHIVE).then(|| Err(ImapError::new("UID FETCH", "candidate read failed")))
    }));
    let error = reconcile_archive_message(&mut client, &request(identity()), &snapshot.source_hash)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("candidate read failed"),
        "{error}"
    );
}

#[tokio::test]
async fn never_treats_a_changed_source_uidvalidity_as_proof_of_move() {
    let server = fixture(true, false);
    let mut client = server.client();
    let snapshot = inspect_archive_source(&mut client, &request(identity()))
        .await
        .unwrap();
    server.put(ARCHIVE, 12, message());
    server.lock().after_select = Some(Box::new(|path, state| {
        if path == "INBOX" {
            state.folder_mut("INBOX").unwrap().uid_validity = 99;
        }
    }));
    let result =
        reconcile_archive_message(&mut client, &request(identity()), &snapshot.source_hash)
            .await
            .unwrap();
    assert_eq!(result, ArchiveReconcileResult::Uncertain);
}

#[tokio::test]
async fn moves_only_the_exact_uid_and_verifies_content_and_flags_in_archive() {
    let server = fixture(true, false);
    let mut client = server.client();
    let snapshot = inspect_archive_source(&mut client, &request(identity()))
        .await
        .unwrap();
    let moved = move_archive_message(&mut client, &request(identity()), &snapshot.source_hash)
        .await
        .unwrap();
    assert!(server.calls().contains(&FakeCall::Move {
        folder: "INBOX".to_owned(),
        uid: 7,
        destination: ARCHIVE.to_owned()
    }));
    assert_eq!(moved.destination, location(ARCHIVE, "20", 12));
    assert!(!server.uids("INBOX").contains(&7));
    assert!(
        verify_archive_location(
            &mut client,
            &moved.destination,
            &identity().message_id,
            &snapshot.source_hash,
            &snapshot.flags
        )
        .await
        .unwrap()
    );
    let reconciled =
        reconcile_archive_message(&mut client, &request(identity()), &snapshot.source_hash)
            .await
            .unwrap();
    assert!(
        matches!(reconciled, ArchiveReconcileResult::Moved { destination, .. } if destination == moved.destination)
    );
    let restored = restore_archive_message(
        &mut client,
        &identity(),
        &moved.destination,
        &snapshot.source_hash,
    )
    .await
    .unwrap();
    assert_eq!(restored.destination.folder, "INBOX");
    assert!(server.uids("INBOX").contains(&8));
}

#[tokio::test]
async fn copies_and_verifies_before_marking_or_expunging_the_exact_source_uid() {
    let server = fixture(false, true);
    let mut client = server.client();
    let snapshot = inspect_archive_source(&mut client, &request(identity()))
        .await
        .unwrap();
    assert_eq!(snapshot.strategy, Some(ArchiveStrategy::UidplusCopy));
    let target = snapshot.target_folder.clone().unwrap();
    let destination =
        copy_exact_archive_message(&mut client, &request(identity()), &target, &snapshot)
            .await
            .unwrap();
    assert!(server.uids("INBOX").contains(&7));
    assert_eq!(stores(&server), 0);
    let reconciled = reconcile_exact_copy(
        &mut client,
        &request(identity()),
        &destination.folder,
        &snapshot,
    )
    .await
    .unwrap();
    assert!(
        matches!(&reconciled, ArchiveReconcileResult::Moved { destination: d, .. } if *d == destination)
    );
    mark_exact_archive_source_deleted(&mut client, &identity(), &destination, &snapshot)
        .await
        .unwrap();
    assert!(server.calls().contains(&FakeCall::Store {
        folder: "INBOX".to_owned(),
        uids: vec![7],
        flags: vec!["\\Deleted".to_owned()],
        silent: false,
    }));
    assert_eq!(
        inspect_exact_deleted_source(&mut client, &identity(), &destination, &snapshot)
            .await
            .unwrap(),
        DeletedSourceState::Marked
    );
    expunge_exact_archive_source(&mut client, &identity(), &destination, &snapshot)
        .await
        .unwrap();
    assert!(server.calls().contains(&FakeCall::Expunge {
        folder: "INBOX".to_owned(),
        uid: 7
    }));
    assert!(!server.uids("INBOX").contains(&7));
    assert!(server.uids(ARCHIVE).contains(&12));
}

#[tokio::test]
async fn never_marks_source_when_copy_fails_or_its_uid_mapping_cannot_verify() {
    let server = fixture(false, true);
    let mut client = server.client();
    let snapshot = inspect_archive_source(&mut client, &request(identity()))
        .await
        .unwrap();
    let target = snapshot.target_folder.clone().unwrap();
    let scripted: Mutex<Vec<Option<CopyUid>>> = Mutex::new(vec![
        Some(CopyUid {
            uid_validity: 20,
            uid_map: Vec::new(),
        }),
        None,
    ]);
    server.lock().copy_override = Some(Box::new(move |op| {
        (op == FakeOp::Copy).then(|| Ok(scripted.lock().unwrap().pop().flatten()))
    }));
    let error = copy_exact_archive_message(&mut client, &request(identity()), &target, &snapshot)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("did not confirm COPY"),
        "{error}"
    );
    assert!(server.uids("INBOX").contains(&7));
    assert_eq!(stores(&server), 0);
    let error = copy_exact_archive_message(&mut client, &request(identity()), &target, &snapshot)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("COPYUID mapping"), "{error}");
    assert_eq!(stores(&server), 0);
}

#[tokio::test]
async fn rejects_source_uid_reuse_or_changed_destination_before_deletion() {
    let server = fixture(false, true);
    let mut client = server.client();
    let snapshot = inspect_archive_source(&mut client, &request(identity()))
        .await
        .unwrap();
    let target = snapshot.target_folder.clone().unwrap();
    let destination =
        copy_exact_archive_message(&mut client, &request(identity()), &target, &snapshot)
            .await
            .unwrap();
    server
        .lock()
        .folder_mut(ARCHIVE)
        .unwrap()
        .messages
        .get_mut(&12)
        .unwrap()
        .source = b"changed".to_vec();
    let error =
        mark_exact_archive_source_deleted(&mut client, &identity(), &destination, &snapshot)
            .await
            .unwrap_err();
    assert!(error.to_string().contains("destination changed"), "{error}");
    assert_eq!(stores(&server), 0);
    server.put(ARCHIVE, 12, message());
    server.put(
        "INBOX",
        7,
        message().envelope_id(Some("<other@example.test>")),
    );
    let error =
        mark_exact_archive_source_deleted(&mut client, &identity(), &destination, &snapshot)
            .await
            .unwrap_err();
    assert!(
        error.to_string().contains("different Message-ID"),
        "{error}"
    );
    assert_eq!(stores(&server), 0);
}

fn sibling() -> ArchiveLocation {
    location(ARCHIVE, "20", 11)
}

fn with_sibling(claimed: Vec<ArchiveLocation>) -> ArchiveSourceRequest {
    ArchiveSourceRequest {
        identity: identity(),
        claimed_copies: claimed,
    }
}

#[tokio::test]
async fn archives_past_a_siblings_claimed_copy_and_verifies_the_new_uid() {
    let server = fixture(true, false);
    server.put(ARCHIVE, 11, message());
    let mut client = server.client();
    let snapshot = inspect_archive_source(&mut client, &with_sibling(vec![sibling()]))
        .await
        .unwrap();
    let moved = move_archive_message(
        &mut client,
        &with_sibling(vec![sibling()]),
        &snapshot.source_hash,
    )
    .await
    .unwrap();
    assert_eq!(moves(&server), 1);
    assert_eq!(moved.destination, location(ARCHIVE, "20", 12));
    let reconciled = reconcile_archive_message(
        &mut client,
        &with_sibling(vec![sibling()]),
        &snapshot.source_hash,
    )
    .await
    .unwrap();
    assert!(
        matches!(&reconciled, ArchiveReconcileResult::Moved { destination, .. } if *destination == moved.destination)
    );
    assert_eq!(
        reconcile_archive_message(&mut client, &request(identity()), &snapshot.source_hash)
            .await
            .unwrap(),
        ArchiveReconcileResult::Uncertain
    );
    assert!(server.uids(ARCHIVE).contains(&11));
}

#[tokio::test]
async fn copies_past_a_siblings_claimed_copy_and_reconciles_to_the_copyuid() {
    let server = fixture(false, true);
    server.put(ARCHIVE, 11, message());
    let mut client = server.client();
    let snapshot = inspect_archive_source(&mut client, &with_sibling(vec![sibling()]))
        .await
        .unwrap();
    let target = snapshot.target_folder.clone().unwrap();
    let copied = copy_exact_archive_message(
        &mut client,
        &with_sibling(vec![sibling()]),
        &target,
        &snapshot,
    )
    .await
    .unwrap();
    assert_eq!(copied, location(ARCHIVE, "20", 12));
    let reconciled = reconcile_exact_copy(
        &mut client,
        &with_sibling(vec![sibling()]),
        &target,
        &snapshot,
    )
    .await
    .unwrap();
    assert!(
        matches!(&reconciled, ArchiveReconcileResult::Moved { destination, .. } if *destination == copied)
    );
}

#[tokio::test]
async fn still_refuses_an_unclaimed_identical_archive_copy() {
    let server = fixture(true, false);
    server.put(ARCHIVE, 11, message());
    let mut client = server.client();
    for claimed in [location(ARCHIVE, "20", 99), location(ARCHIVE, "19", 11)] {
        let error = move_archive_message(&mut client, &with_sibling(vec![claimed]), "unused-hash")
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Matching Archive copy already exists"),
            "{error}"
        );
    }
    assert_eq!(moves(&server), 0);
}
