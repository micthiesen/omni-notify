//! Exact IMAP archive operations: native MOVE
//! of one verified Inbox UID to the designated Archive, the UIDPLUS
//! COPY + `\Deleted` + `UID EXPUNGE <uid>` fallback (one mutation per call),
//! and read-only reconciliation after a lost response. Never a mailbox-wide
//! EXPUNGE.

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::protocol::{
    FetchQuery, ImapClient, ImapError, SearchCriteria, SourceRange, fetch_one, selected_validity,
};

const MAX_ARCHIVE_SOURCE_BYTES: usize = 20 * 1024 * 1024;

/// One physical message copy: mailbox, UIDVALIDITY and UID.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveLocation {
    pub folder: String,
    pub uid_validity: String,
    pub uid: u32,
}

impl ArchiveLocation {
    pub fn same(&self, other: &ArchiveLocation) -> bool {
        self.folder == other.folder
            && self.uid_validity == other.uid_validity
            && self.uid == other.uid
    }

    pub fn with_message_id(&self, message_id: &str) -> ArchiveIdentity {
        ArchiveIdentity {
            folder: self.folder.clone(),
            uid_validity: self.uid_validity.clone(),
            uid: self.uid,
            message_id: message_id.to_owned(),
        }
    }
}

/// A location plus the exact bracketed Message-ID it must carry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveIdentity {
    pub folder: String,
    pub uid_validity: String,
    pub uid: u32,
    pub message_id: String,
}

impl ArchiveIdentity {
    pub fn location(&self) -> ArchiveLocation {
        ArchiveLocation {
            folder: self.folder.clone(),
            uid_validity: self.uid_validity.clone(),
            uid: self.uid,
        }
    }

    pub fn same_copy(&self, other: &ArchiveIdentity) -> bool {
        self.folder == other.folder
            && self.uid_validity == other.uid_validity
            && self.uid == other.uid
    }
}

/// An archive source with call-time context, never persisted. `claimed_copies`
/// are verified Archive destinations of other settled actions for the same
/// Message-ID: identical duplicate deliveries share MIME bytes, so those copies
/// are not evidence of an earlier move of this source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveSourceRequest {
    pub identity: ArchiveIdentity,
    pub claimed_copies: Vec<ArchiveLocation>,
}

impl From<ArchiveIdentity> for ArchiveSourceRequest {
    fn from(identity: ArchiveIdentity) -> Self {
        Self {
            identity,
            claimed_copies: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArchiveStrategy {
    #[serde(rename = "uidplus_copy")]
    UidplusCopy,
}

/// Content hash and flags of an exact copy (and, for UIDPLUS, the plan).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveSnapshot {
    pub source_hash: String,
    pub flags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strategy: Option<ArchiveStrategy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_folder: Option<String>,
}

impl ArchiveSnapshot {
    fn plain(source_hash: String, flags: Vec<String>) -> Self {
        Self {
            source_hash,
            flags,
            strategy: None,
            target_folder: None,
        }
    }

    pub fn is_uidplus(&self) -> bool {
        self.strategy == Some(ArchiveStrategy::UidplusCopy)
    }

    fn has_deleted(&self) -> bool {
        self.flags
            .iter()
            .any(|f| f.eq_ignore_ascii_case("\\deleted"))
    }

    fn flags_without_deleted(&self) -> Vec<String> {
        self.flags
            .iter()
            .filter(|f| !f.eq_ignore_ascii_case("\\deleted"))
            .cloned()
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveMoveResult {
    pub destination: ArchiveLocation,
    pub snapshot: ArchiveSnapshot,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArchiveReconcileResult {
    Moved {
        destination: ArchiveLocation,
        snapshot: ArchiveSnapshot,
    },
    NotMoved,
    Uncertain,
}

/// The observed state of an exact source after `\Deleted` was claimed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeletedSourceState {
    Marked,
    Unmarked,
    Absent,
    Uncertain,
}

fn fail<T>(operation: &str, detail: &str) -> Result<T, ImapError> {
    Err(ImapError::new(operation, detail))
}

fn content_hash(source: &[u8]) -> String {
    hex::encode(Sha256::digest(source))
}

/// `\Recent` is session-local and cannot be preserved across mailboxes.
fn normalized_flags(flags: &[String]) -> Vec<String> {
    let mut out: Vec<String> = flags
        .iter()
        .filter(|f| !f.eq_ignore_ascii_case("\\recent"))
        .cloned()
        .collect();
    // JS default sort: UTF-16 code unit order (ASCII flags compare bytewise).
    out.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
    out.dedup();
    out
}

fn sorted(flags: &[String]) -> Vec<String> {
    let mut out = flags.to_vec();
    out.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
    out
}

fn has_cap(client: &dyn ImapClient, name: &str) -> bool {
    client.has_capability(name)
}

fn exact_query() -> FetchQuery {
    FetchQuery {
        source: Some(SourceRange::Full),
        flags: true,
        envelope: true,
        ..FetchQuery::default()
    }
}

/// Reads one exact UID in the selected mailbox.
async fn read_exact(
    client: &mut dyn ImapClient,
    identity: &ArchiveIdentity,
) -> Result<Option<ArchiveSnapshot>, ImapError> {
    if selected_validity(client).as_deref() != Some(identity.uid_validity.as_str()) {
        return fail("verify mailbox identity", "UIDVALIDITY changed");
    }
    let message = fetch_one(client, identity.uid, exact_query())
        .await
        .map_err(|e| ImapError::wrap("read exact message", e))?;
    let Some(message) = message else {
        return Ok(None);
    };
    if message.envelope_message_id.as_deref() != Some(identity.message_id.as_str()) {
        return fail(
            "verify message identity",
            "UID belongs to a different Message-ID",
        );
    }
    let (Some(source), Some(flags)) = (message.source, message.flags) else {
        return fail("read exact message", "MIME source or flags unavailable");
    };
    if source.len() > MAX_ARCHIVE_SOURCE_BYTES {
        return fail("read exact message", "MIME source exceeds archive limit");
    }
    Ok(Some(ArchiveSnapshot::plain(
        content_hash(&source),
        normalized_flags(&flags),
    )))
}

async fn select(
    client: &mut dyn ImapClient,
    folder: &str,
    read_only: bool,
) -> Result<(), ImapError> {
    client
        .select(folder, read_only)
        .await
        .map_err(|e| ImapError::wrap(format!("select {folder}"), e))
}

/// The one selectable designated Archive (never Inbox).
pub async fn discover_archive(client: &mut dyn ImapClient) -> Result<String, ImapError> {
    let folders = client
        .list()
        .await
        .map_err(|e| ImapError::wrap("discover Archive", e))?;
    let archives: Vec<_> = folders
        .iter()
        .filter(|f| f.special_use_is("\\archive") && !f.has_flag("\\Noselect"))
        .collect();
    if archives.len() != 1 {
        return fail(
            "discover Archive",
            "Expected one selectable designated Archive",
        );
    }
    if archives[0].path == "INBOX" {
        return fail("discover Archive", "Archive resolves to Inbox");
    }
    Ok(archives[0].path.clone())
}

/// Exact source snapshot before the durable MOVE/COPY claim.
pub async fn inspect_archive_source(
    client: &mut dyn ImapClient,
    request: &ArchiveSourceRequest,
) -> Result<ArchiveSnapshot, ImapError> {
    let identity = &request.identity;
    if identity.folder != "INBOX" {
        return fail("inspect archive source", "Only Inbox mail can be archived");
    }
    if !has_cap(client, "MOVE") && !has_cap(client, "UIDPLUS") {
        return fail("safe move", "Server advertises neither MOVE nor UIDPLUS");
    }
    let archive = discover_archive(client).await?;
    select(client, "INBOX", true).await?;
    let Some(snapshot) = read_exact(client, identity).await? else {
        return fail("inspect archive source", "Inbox UID is gone");
    };
    if !has_cap(client, "MOVE") && snapshot.has_deleted() {
        return fail("inspect archive source", "Inbox source is already Deleted");
    }
    let existing = find_exact_in_folder(
        client,
        &archive,
        &identity.message_id,
        &snapshot.source_hash,
        &request.claimed_copies,
    )
    .await?;
    if !existing.is_empty() {
        return fail(
            "inspect archive destination",
            "Matching Archive copy already exists",
        );
    }
    if has_cap(client, "MOVE") {
        Ok(snapshot)
    } else {
        Ok(ArchiveSnapshot {
            strategy: Some(ArchiveStrategy::UidplusCopy),
            target_folder: Some(archive),
            ..snapshot
        })
    }
}

async fn move_exact(
    client: &mut dyn ImapClient,
    source: &ArchiveIdentity,
    destination_folder: &str,
    expected_hash: &str,
) -> Result<ArchiveMoveResult, ImapError> {
    // Without MOVE a client would emulate it with COPY + \Deleted + EXPUNGE.
    if !has_cap(client, "MOVE") {
        return fail("native MOVE", "Server does not advertise MOVE");
    }
    select(client, &source.folder, false).await?;
    let snapshot = read_exact(client, source).await?;
    let Some(snapshot) = snapshot.filter(|s| s.source_hash == expected_hash) else {
        return fail("verify source before MOVE", "Source changed or disappeared");
    };
    let moved = client
        .uid_move(source.uid, destination_folder)
        .await
        .map_err(|e| ImapError::wrap("native MOVE", e))?;
    let Some(moved) = moved else {
        return fail("native MOVE", "Server did not confirm MOVE");
    };
    let uid = moved.destination_of(source.uid).filter(|uid| *uid > 0);
    let Some(uid) = uid.filter(|_| moved.uid_validity > 0) else {
        return fail("verify MOVE", "Server supplied no destination UID mapping");
    };
    Ok(ArchiveMoveResult {
        destination: ArchiveLocation {
            folder: destination_folder.to_owned(),
            uid_validity: moved.uid_validity.to_string(),
            uid,
        },
        snapshot,
    })
}

/// One native MOVE of the exact Inbox UID to the designated Archive.
pub async fn move_archive_message(
    client: &mut dyn ImapClient,
    request: &ArchiveSourceRequest,
    expected_hash: &str,
) -> Result<ArchiveMoveResult, ImapError> {
    if request.identity.folder != "INBOX" {
        return fail("archive message", "Only Inbox mail can be archived");
    }
    let snapshot = inspect_archive_source(client, request).await?;
    if snapshot.source_hash != expected_hash {
        return fail("archive message", "Source changed since reservation");
    }
    let archive = discover_archive(client).await?;
    move_exact(client, &request.identity, &archive, expected_hash).await
}

/// Reverses one recorded archive move by exact Archive coordinates.
pub async fn restore_archive_message(
    client: &mut dyn ImapClient,
    original: &ArchiveIdentity,
    destination: &ArchiveLocation,
    expected_hash: &str,
) -> Result<ArchiveMoveResult, ImapError> {
    if original.folder != "INBOX" {
        return fail("restore archive message", "Original mailbox was not Inbox");
    }
    let archive = discover_archive(client).await?;
    if destination.folder != archive {
        return fail(
            "restore archive message",
            "Recorded destination is not Archive",
        );
    }
    let existing_inbox =
        find_exact_in_folder(client, "INBOX", &original.message_id, expected_hash, &[]).await?;
    if !existing_inbox.is_empty() {
        return fail(
            "restore archive message",
            "Matching Inbox copy already exists",
        );
    }
    move_exact(
        client,
        &destination.with_message_id(&original.message_id),
        "INBOX",
        expected_hash,
    )
    .await
}

/// Checks the recorded Archive UID without changing any flag.
pub async fn inspect_archive_destination(
    client: &mut dyn ImapClient,
    original: &ArchiveIdentity,
    destination: &ArchiveLocation,
) -> Result<ArchiveSnapshot, ImapError> {
    let archive = discover_archive(client).await?;
    if destination.folder != archive {
        return fail(
            "inspect restore source",
            "Recorded destination is not Archive",
        );
    }
    if !has_cap(client, "MOVE") && !has_cap(client, "UIDPLUS") {
        return fail(
            "restore archive message",
            "Neither MOVE nor UIDPLUS is available",
        );
    }
    select(client, &archive, true).await?;
    let Some(snapshot) =
        read_exact(client, &destination.with_message_id(&original.message_id)).await?
    else {
        return fail("inspect restore source", "Archived UID is gone");
    };
    if !has_cap(client, "MOVE") && snapshot.has_deleted() {
        return fail("inspect restore source", "Archived UID is already Deleted");
    }
    if has_cap(client, "MOVE") {
        Ok(snapshot)
    } else {
        Ok(ArchiveSnapshot {
            strategy: Some(ArchiveStrategy::UidplusCopy),
            target_folder: Some("INBOX".to_owned()),
            ..snapshot
        })
    }
}

/// Exact Message-ID + content matches in `folder`, excluding claimed copies.
async fn find_exact_in_folder(
    client: &mut dyn ImapClient,
    folder: &str,
    message_id: &str,
    expected_hash: &str,
    claimed_copies: &[ArchiveLocation],
) -> Result<Vec<ArchiveMoveResult>, ImapError> {
    select(client, folder, true).await?;
    let Some(validity) = selected_validity(client) else {
        return fail("inspect mailbox", "UIDVALIDITY unavailable");
    };
    let candidates = client
        .uid_search(&SearchCriteria::message_id(message_id))
        .await
        .map_err(|e| ImapError::wrap("find exact Message-ID", e))?;
    let Some(candidates) = candidates else {
        return fail("find exact Message-ID", "IMAP search was not confirmed");
    };
    if candidates.len() > 50 {
        return fail("find exact Message-ID", "Too many candidate messages");
    }
    let mut matches = Vec::new();
    for uid in candidates {
        let message = fetch_one(client, uid, exact_query())
            .await
            .map_err(|e| ImapError::wrap("read Message-ID candidate", e))?;
        let Some(message) = message else {
            return fail("read Message-ID candidate", "Candidate UID vanished");
        };
        // HEADER search is substring matching; only this mismatch is safe to skip.
        if message.envelope_message_id.as_deref() != Some(message_id) {
            continue;
        }
        let (Some(source), Some(flags)) = (message.source, message.flags) else {
            return fail(
                "read Message-ID candidate",
                "MIME source or flags unavailable",
            );
        };
        if source.len() > MAX_ARCHIVE_SOURCE_BYTES {
            return fail(
                "read Message-ID candidate",
                "MIME source exceeds archive limit",
            );
        }
        let snapshot = ArchiveSnapshot::plain(content_hash(&source), normalized_flags(&flags));
        let destination = ArchiveLocation {
            folder: folder.to_owned(),
            uid_validity: validity.clone(),
            uid,
        };
        if snapshot.source_hash == expected_hash
            && !claimed_copies
                .iter()
                .any(|claimed| claimed.same(&destination))
        {
            matches.push(ArchiveMoveResult {
                destination,
                snapshot,
            });
        }
    }
    Ok(matches)
}

/// No mutation: reconciles a possibly lost MOVE response.
pub async fn reconcile_archive_message(
    client: &mut dyn ImapClient,
    request: &ArchiveSourceRequest,
    source_hash: &str,
) -> Result<ArchiveReconcileResult, ImapError> {
    let archive = discover_archive(client).await?;
    let source = match select(client, "INBOX", true).await {
        Ok(()) => read_exact(client, &request.identity).await,
        Err(e) => Err(e),
    };
    let Ok(source) = source else {
        return Ok(ArchiveReconcileResult::Uncertain);
    };
    let destinations = find_exact_in_folder(
        client,
        &archive,
        &request.identity.message_id,
        source_hash,
        &request.claimed_copies,
    )
    .await?;
    if source
        .as_ref()
        .is_some_and(|s| s.source_hash == source_hash)
        && destinations.is_empty()
    {
        return Ok(ArchiveReconcileResult::NotMoved);
    }
    if source.is_none() && destinations.len() == 1 {
        let found = destinations.into_iter().next();
        if let Some(found) = found {
            return Ok(ArchiveReconcileResult::Moved {
                destination: found.destination,
                snapshot: found.snapshot,
            });
        }
    }
    Ok(ArchiveReconcileResult::Uncertain)
}

/// Read-only verification of an exact UID returned by COPYUID.
pub async fn verify_archive_location(
    client: &mut dyn ImapClient,
    location: &ArchiveLocation,
    message_id: &str,
    source_hash: &str,
    expected_flags: &[String],
) -> Result<bool, ImapError> {
    select(client, &location.folder, true).await?;
    let snapshot = read_exact(client, &location.with_message_id(message_id)).await?;
    Ok(snapshot.is_some_and(|s| s.source_hash == source_hash && s.flags == sorted(expected_flags)))
}

/// After a lost restore response, inspects both folders without repeating MOVE.
pub async fn reconcile_restore_message(
    client: &mut dyn ImapClient,
    original: &ArchiveIdentity,
    destination: &ArchiveLocation,
    source_hash: &str,
) -> Result<ArchiveReconcileResult, ImapError> {
    let archive = discover_archive(client).await?;
    if destination.folder != archive {
        return fail("reconcile restore", "Recorded destination is not Archive");
    }
    let archived = match select(client, &archive, true).await {
        Ok(()) => read_exact(client, &destination.with_message_id(&original.message_id)).await,
        Err(e) => Err(e),
    };
    let Ok(archived) = archived else {
        return Ok(ArchiveReconcileResult::Uncertain);
    };
    let inbox =
        find_exact_in_folder(client, "INBOX", &original.message_id, source_hash, &[]).await?;
    if archived.is_none() && inbox.len() == 1 {
        if let Some(found) = inbox.into_iter().next() {
            return Ok(ArchiveReconcileResult::Moved {
                destination: found.destination,
                snapshot: found.snapshot,
            });
        }
        return Ok(ArchiveReconcileResult::Uncertain);
    }
    if archived.is_some_and(|a| a.source_hash == source_hash) && inbox.is_empty() {
        return Ok(ArchiveReconcileResult::NotMoved);
    }
    Ok(ArchiveReconcileResult::Uncertain)
}

fn inside_archive_action(source: &ArchiveIdentity, target_folder: &str, archive: &str) -> bool {
    (source.folder == "INBOX" && target_folder == archive)
        || (source.folder == archive && target_folder == "INBOX")
}

/// UIDPLUS path step 1: one UID COPY of the verified source.
pub async fn copy_exact_archive_message(
    client: &mut dyn ImapClient,
    request: &ArchiveSourceRequest,
    target_folder: &str,
    snapshot: &ArchiveSnapshot,
) -> Result<ArchiveLocation, ImapError> {
    let source = &request.identity;
    let archive = discover_archive(client).await?;
    if !inside_archive_action(source, target_folder, &archive) {
        return fail("UID COPY", "Source or target is outside the archive action");
    }
    if !has_cap(client, "UIDPLUS") {
        return fail("UID COPY", "UIDPLUS COPY is unavailable");
    }
    let existing = find_exact_in_folder(
        client,
        target_folder,
        &source.message_id,
        &snapshot.source_hash,
        &request.claimed_copies,
    )
    .await?;
    if !existing.is_empty() {
        return fail("UID COPY", "Matching destination already exists");
    }
    select(client, &source.folder, false).await?;
    let current = read_exact(client, source).await?;
    let unchanged = current.as_ref().is_some_and(|c| {
        c.source_hash == snapshot.source_hash && c.flags == snapshot.flags && !c.has_deleted()
    });
    if !unchanged {
        return fail("UID COPY", "Source changed or is already Deleted");
    }
    let copied = client
        .uid_copy(source.uid, target_folder)
        .await
        .map_err(|e| ImapError::wrap("UID COPY", e))?;
    let Some(copied) = copied else {
        return fail("UID COPY", "Server did not confirm COPY");
    };
    let uid = copied.destination_of(source.uid).filter(|uid| *uid > 0);
    let Some(uid) = uid.filter(|_| copied.uid_validity > 0) else {
        return fail("UID COPY", "Server supplied no COPYUID mapping");
    };
    Ok(ArchiveLocation {
        folder: target_folder.to_owned(),
        uid_validity: copied.uid_validity.to_string(),
        uid,
    })
}

/// Read-only recovery after a lost COPY response; never a second COPY.
pub async fn reconcile_exact_copy(
    client: &mut dyn ImapClient,
    request: &ArchiveSourceRequest,
    target_folder: &str,
    snapshot: &ArchiveSnapshot,
) -> Result<ArchiveReconcileResult, ImapError> {
    let source = match select(client, &request.identity.folder, true).await {
        Ok(()) => read_exact(client, &request.identity).await,
        Err(e) => Err(e),
    };
    match source {
        Ok(Some(s)) if s.source_hash == snapshot.source_hash => {}
        _ => return Ok(ArchiveReconcileResult::Uncertain),
    }
    let matches = find_exact_in_folder(
        client,
        target_folder,
        &request.identity.message_id,
        &snapshot.source_hash,
        &request.claimed_copies,
    )
    .await?;
    if matches.len() != 1 || matches[0].snapshot.flags != snapshot.flags {
        return Ok(ArchiveReconcileResult::Uncertain);
    }
    let found = matches.into_iter().next();
    Ok(found.map_or(ArchiveReconcileResult::Uncertain, |found| {
        ArchiveReconcileResult::Moved {
            destination: found.destination,
            snapshot: found.snapshot,
        }
    }))
}

/// Verifies both copies, then marks only the exact source UID `\Deleted`.
pub async fn mark_exact_archive_source_deleted(
    client: &mut dyn ImapClient,
    source: &ArchiveIdentity,
    destination: &ArchiveLocation,
    snapshot: &ArchiveSnapshot,
) -> Result<bool, ImapError> {
    let archive = discover_archive(client).await?;
    if !inside_archive_action(source, &destination.folder, &archive) {
        return fail("UID STORE", "Archive mailbox identity changed");
    }
    if !has_cap(client, "UIDPLUS") {
        return fail("UID STORE", "UIDPLUS STORE is unavailable");
    }
    let verified = verify_archive_location(
        client,
        destination,
        &source.message_id,
        &snapshot.source_hash,
        &snapshot.flags,
    )
    .await?;
    if !verified {
        return fail("UID STORE", "Verified destination changed");
    }
    select(client, &source.folder, false).await?;
    let current = read_exact(client, source).await?;
    let unchanged = current.as_ref().is_some_and(|c| {
        c.source_hash == snapshot.source_hash && c.flags == snapshot.flags && !c.has_deleted()
    });
    if !unchanged {
        return fail("UID STORE", "Exact source changed before deletion");
    }
    let marked = client
        .uid_store_add_flags(&[source.uid], &["\\Deleted"], false)
        .await
        .map_err(|e| ImapError::wrap("UID STORE Deleted", e))?;
    if !marked {
        return fail("UID STORE", "Server did not confirm Deleted flag");
    }
    Ok(true)
}

/// A lost STORE response is reconciled by reading the exact UID and flags.
pub async fn inspect_exact_deleted_source(
    client: &mut dyn ImapClient,
    source: &ArchiveIdentity,
    destination: &ArchiveLocation,
    snapshot: &ArchiveSnapshot,
) -> Result<DeletedSourceState, ImapError> {
    let verified = verify_archive_location(
        client,
        destination,
        &source.message_id,
        &snapshot.source_hash,
        &snapshot.flags,
    )
    .await?;
    if !verified {
        return Ok(DeletedSourceState::Uncertain);
    }
    let current = match select(client, &source.folder, true).await {
        Ok(()) => read_exact(client, source).await,
        Err(e) => Err(e),
    };
    let Ok(current) = current else {
        return Ok(DeletedSourceState::Uncertain);
    };
    let Some(current) = current else {
        return Ok(DeletedSourceState::Absent);
    };
    if current.source_hash != snapshot.source_hash {
        return Ok(DeletedSourceState::Uncertain);
    }
    if current.flags_without_deleted() != snapshot.flags {
        return Ok(DeletedSourceState::Uncertain);
    }
    Ok(if current.has_deleted() {
        DeletedSourceState::Marked
    } else {
        DeletedSourceState::Unmarked
    })
}

/// Only `UID EXPUNGE <uid>` of the exact source UID.
pub async fn expunge_exact_archive_source(
    client: &mut dyn ImapClient,
    source: &ArchiveIdentity,
    destination: &ArchiveLocation,
    snapshot: &ArchiveSnapshot,
) -> Result<bool, ImapError> {
    let archive = discover_archive(client).await?;
    if !inside_archive_action(source, &destination.folder, &archive) {
        return fail("UID EXPUNGE", "Archive mailbox identity changed");
    }
    if !has_cap(client, "UIDPLUS") {
        return fail("UID EXPUNGE", "Exact UID EXPUNGE unavailable");
    }
    let observed = inspect_exact_deleted_source(client, source, destination, snapshot).await?;
    if observed != DeletedSourceState::Marked {
        return fail("UID EXPUNGE", "Source or destination changed");
    }
    select(client, &source.folder, false).await?;
    // Re-read under the mutation selection to reject UIDVALIDITY change or reuse.
    let current = read_exact(client, source).await?;
    let unchanged = current.as_ref().is_some_and(|c| {
        c.source_hash == snapshot.source_hash
            && c.has_deleted()
            && c.flags_without_deleted() == snapshot.flags
    });
    if !unchanged {
        return fail("UID EXPUNGE", "Exact source changed");
    }
    let removed = client
        .uid_expunge(source.uid)
        .await
        .map_err(|e| ImapError::wrap("UID EXPUNGE", e))?;
    if !removed {
        return fail("UID EXPUNGE", "Server did not confirm UID EXPUNGE");
    }
    Ok(true)
}
