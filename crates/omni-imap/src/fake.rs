//! An in-memory IMAP server model implementing [`ImapClient`] for tests
//! (feature `testing`). Folders hold messages by UID with flags, INTERNALDATE
//! and an ENVELOPE Message-ID taken from the raw header; every client call is
//! logged, and hooks inject failures, overrides and pauses.

use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::BoxFuture;
use tokio::sync::Notify;

use crate::protocol::connect::Connector;
use crate::protocol::{
    CopyUid, FetchQuery, FetchedMessage, FolderStatus, IdleEnd, ImapClient, ImapError, ImapResult,
    MailboxEvent, MailboxInfo, SearchCriteria, SelectedMailbox, SourceRange, UidSet,
};

/// One stored message.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FakeMessage {
    pub source: Vec<u8>,
    pub flags: Vec<String>,
    pub internal_date_ms: Option<i64>,
    /// ENVELOPE Message-ID; defaults to the raw `Message-ID` header value.
    pub envelope_message_id: Option<String>,
}

impl FakeMessage {
    pub fn new(source: impl Into<Vec<u8>>) -> Self {
        let source = source.into();
        let envelope_message_id = raw_message_id(&source);
        Self {
            source,
            flags: Vec::new(),
            internal_date_ms: None,
            envelope_message_id,
        }
    }

    pub fn flags(mut self, flags: &[&str]) -> Self {
        self.flags = flags.iter().map(|f| (*f).to_owned()).collect();
        self
    }

    pub fn date(mut self, ms: i64) -> Self {
        self.internal_date_ms = Some(ms);
        self
    }

    pub fn envelope_id(mut self, id: Option<&str>) -> Self {
        self.envelope_message_id = id.map(str::to_owned);
        self
    }
}

/// The unfolded, trimmed `Message-ID` header value (what an ENVELOPE carries).
pub fn raw_message_id(source: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(source);
    let head = text.split("\r\n\r\n").next().unwrap_or("");
    let head = head.split("\n\n").next().unwrap_or(head);
    let mut value: Option<String> = None;
    for line in head.lines() {
        if line.starts_with([' ', '\t']) {
            if let Some(v) = value.as_mut() {
                v.push_str(line.trim());
            }
            continue;
        }
        if let Some(found) = value.take() {
            return Some(found.trim().to_owned());
        }
        if let Some((name, rest)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("message-id")
        {
            value = Some(rest.trim().to_owned());
        }
    }
    value.map(|v| v.trim().to_owned())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FakeFolder {
    pub path: String,
    pub uid_validity: u32,
    pub uid_next: u32,
    pub special_use: Option<String>,
    pub flags: Vec<String>,
    pub messages: BTreeMap<u32, FakeMessage>,
}

/// A logged client call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FakeCall {
    List,
    Select {
        path: String,
        read_only: bool,
    },
    Status {
        path: String,
    },
    Search {
        folder: String,
        criteria: SearchCriteria,
    },
    Fetch {
        folder: String,
        uids: UidSet,
        query: FetchQuery,
    },
    Move {
        folder: String,
        uid: u32,
        destination: String,
    },
    Copy {
        folder: String,
        uid: u32,
        destination: String,
    },
    Store {
        folder: String,
        uids: Vec<u32>,
        flags: Vec<String>,
        silent: bool,
    },
    Expunge {
        folder: String,
        uid: u32,
    },
    Append {
        path: String,
        flags: Vec<String>,
        internal_date_ms: Option<i64>,
    },
    Idle,
    Logout,
}

/// Which call a hook targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FakeOp {
    List,
    Select,
    Status,
    Search,
    Fetch,
    Move,
    Copy,
    Store,
    Expunge,
    Append,
}

type SearchOverride =
    Box<dyn Fn(&str, &SearchCriteria) -> Option<ImapResult<Option<Vec<u32>>>> + Send>;
type FetchOverride =
    Box<dyn Fn(&str, &UidSet, &FetchQuery) -> Option<ImapResult<Vec<FetchedMessage>>> + Send>;
type SelectHook = Box<dyn Fn(&str, &mut FakeState) + Send>;
type BoolOverride = Box<dyn Fn(FakeOp, &mut FakeState) -> Option<ImapResult<bool>> + Send>;
type CopyOverride = Box<dyn Fn(FakeOp) -> Option<ImapResult<Option<CopyUid>>> + Send>;

pub struct FakeState {
    pub folders: Vec<FakeFolder>,
    pub capabilities: HashSet<String>,
    pub calls: Vec<FakeCall>,
    failures: Vec<(FakeOp, Option<String>, ImapError)>,
    pub search_override: Option<SearchOverride>,
    pub fetch_override: Option<FetchOverride>,
    pub after_select: Option<SelectHook>,
    pub bool_override: Option<BoolOverride>,
    pub copy_override: Option<CopyOverride>,
    /// Calls of these kinds wait on `gate` before running.
    pub paused: HashSet<FakeOp>,
    pub entered: Arc<Notify>,
    pub gate: Arc<Notify>,
    pub connect_failures: Vec<ImapError>,
    pub connects: usize,
    pub connect_gate: Option<Arc<Notify>>,
    /// Bumped by [`FakeServer::disconnect_all`]; older clients become unusable.
    pub epoch: u64,
}

impl FakeState {
    pub fn folder(&self, path: &str) -> Option<&FakeFolder> {
        self.folders.iter().find(|f| f.path == path)
    }

    pub fn folder_mut(&mut self, path: &str) -> Option<&mut FakeFolder> {
        self.folders.iter_mut().find(|f| f.path == path)
    }

    fn take_failure(&mut self, op: FakeOp, folder: Option<&str>) -> Option<ImapError> {
        let index = self
            .failures
            .iter()
            .position(|(o, f, _)| *o == op && f.as_deref().is_none_or(|f| Some(f) == folder))?;
        Some(self.failures.remove(index).2)
    }
}

/// The shared server; clients see each other's writes.
#[derive(Clone)]
pub struct FakeServer {
    pub state: Arc<Mutex<FakeState>>,
}

impl Default for FakeServer {
    fn default() -> Self {
        Self::new(&["IMAP4REV1", "IDLE", "MOVE", "UIDPLUS"])
    }
}

impl FakeServer {
    pub fn new(capabilities: &[&str]) -> Self {
        Self {
            state: Arc::new(Mutex::new(FakeState {
                folders: Vec::new(),
                capabilities: capabilities
                    .iter()
                    .map(|c| c.to_ascii_uppercase())
                    .collect(),
                calls: Vec::new(),
                failures: Vec::new(),
                search_override: None,
                fetch_override: None,
                after_select: None,
                bool_override: None,
                copy_override: None,
                paused: HashSet::new(),
                entered: Arc::new(Notify::new()),
                gate: Arc::new(Notify::new()),
                connect_failures: Vec::new(),
                connects: 0,
                connect_gate: None,
                epoch: 0,
            })),
        }
    }

    pub fn lock(&self) -> std::sync::MutexGuard<'_, FakeState> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Adds a folder (`special_use` like `\Sent`).
    pub fn folder(&self, path: &str, uid_validity: u32, special_use: Option<&str>) -> &Self {
        self.lock().folders.push(FakeFolder {
            path: path.to_owned(),
            uid_validity,
            uid_next: 1,
            special_use: special_use.map(str::to_owned),
            flags: Vec::new(),
            messages: BTreeMap::new(),
        });
        self
    }

    /// Stores a message at `uid` (bumping uidNext past it).
    pub fn put(&self, path: &str, uid: u32, message: FakeMessage) -> &Self {
        let mut state = self.lock();
        if let Some(folder) = state.folder_mut(path) {
            folder.messages.insert(uid, message);
            folder.uid_next = folder.uid_next.max(uid + 1);
        }
        self
    }

    pub fn message(&self, path: &str, uid: u32) -> Option<FakeMessage> {
        self.lock()
            .folder(path)
            .and_then(|f| f.messages.get(&uid).cloned())
    }

    pub fn uids(&self, path: &str) -> Vec<u32> {
        self.lock()
            .folder(path)
            .map(|f| f.messages.keys().copied().collect())
            .unwrap_or_default()
    }

    pub fn calls(&self) -> Vec<FakeCall> {
        self.lock().calls.clone()
    }

    pub fn clear_calls(&self) {
        self.lock().calls.clear();
    }

    pub fn count(&self, matches: impl Fn(&FakeCall) -> bool) -> usize {
        self.lock().calls.iter().filter(|c| matches(c)).count()
    }

    /// The next `op` (optionally only in `folder`) fails with `error`.
    pub fn fail_next(&self, op: FakeOp, folder: Option<&str>, error: ImapError) {
        self.lock()
            .failures
            .push((op, folder.map(str::to_owned), error));
    }

    /// Drops every open session (as a network failure would).
    pub fn disconnect_all(&self) {
        self.lock().epoch += 1;
    }

    pub fn client(&self) -> FakeClient {
        let (capabilities, epoch) = {
            let state = self.lock();
            (state.capabilities.clone(), state.epoch)
        };
        FakeClient {
            server: self.clone(),
            epoch,
            capabilities,
            selected: None,
            usable: true,
            events: Vec::new(),
        }
    }

    pub fn connector(&self) -> Arc<FakeConnector> {
        Arc::new(FakeConnector {
            server: self.clone(),
        })
    }
}

/// Opens [`FakeClient`]s, failing scripted attempts first.
pub struct FakeConnector {
    pub server: FakeServer,
}

impl Connector for FakeConnector {
    fn connect(&self) -> BoxFuture<'_, Result<Box<dyn ImapClient>, ImapError>> {
        Box::pin(async move {
            let gate = {
                let mut state = self.server.lock();
                state.connects += 1;
                state.connect_gate.clone()
            };
            if let Some(gate) = gate {
                gate.notified().await;
            }
            let failure = {
                let mut state = self.server.lock();
                if state.connect_failures.is_empty() {
                    None
                } else {
                    Some(state.connect_failures.remove(0))
                }
            };
            if let Some(error) = failure {
                return Err(error);
            }
            Ok(Box::new(self.server.client()) as Box<dyn ImapClient>)
        })
    }
}

/// One session against the shared [`FakeServer`].
pub struct FakeClient {
    server: FakeServer,
    epoch: u64,
    capabilities: HashSet<String>,
    selected: Option<SelectedMailbox>,
    usable: bool,
    events: Vec<MailboxEvent>,
}

impl FakeClient {
    /// Marks the connection dead (as a dropped socket would).
    pub fn kill(&mut self) {
        self.usable = false;
    }

    fn folder_name(&self) -> String {
        self.selected
            .as_ref()
            .map(|s| s.path.clone())
            .unwrap_or_default()
    }

    async fn enter(&self, op: FakeOp, call: FakeCall) -> ImapResult<()> {
        let (paused, entered, gate) = {
            let mut state = self.server.lock();
            state.calls.push(call);
            (
                state.paused.contains(&op),
                state.entered.clone(),
                state.gate.clone(),
            )
        };
        if paused {
            entered.notify_one();
            gate.notified().await;
        }
        let folder = self.folder_name();
        let failure = self.server.lock().take_failure(op, Some(&folder));
        match failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

/// Runs the bool override with access to the state (taken out while it runs).
fn run_bool_override(state: &mut FakeState, op: FakeOp) -> Option<ImapResult<bool>> {
    let hook = state.bool_override.take()?;
    let result = hook(op, state);
    state.bool_override = Some(hook);
    result
}

fn contains_ci(haystack: &[u8], needle: &str) -> bool {
    String::from_utf8_lossy(haystack)
        .to_lowercase()
        .contains(&needle.to_lowercase())
}

fn day_start(ms: i64) -> i64 {
    ms.div_euclid(86_400_000) * 86_400_000
}

fn matches(message: &FakeMessage, criteria: &SearchCriteria) -> bool {
    if let Some((name, value)) = &criteria.header {
        if name.eq_ignore_ascii_case("message-id") {
            let Some(id) = &message.envelope_message_id else {
                return false;
            };
            if !id.to_lowercase().contains(&value.to_lowercase()) {
                return false;
            }
        } else if !contains_ci(&message.source, value) {
            return false;
        }
    }
    if let Some(seen) = criteria.seen {
        let has = message
            .flags
            .iter()
            .any(|f| f.eq_ignore_ascii_case("\\Seen"));
        if has != seen {
            return false;
        }
    }
    if let Some(since) = criteria.since_ms
        && message
            .internal_date_ms
            .is_none_or(|d| d < day_start(since))
    {
        return false;
    }
    if let Some(before) = criteria.before_ms
        && message
            .internal_date_ms
            .is_none_or(|d| d >= day_start(before))
    {
        return false;
    }
    for value in [
        &criteria.text,
        &criteria.from,
        &criteria.to,
        &criteria.subject,
    ]
    .into_iter()
    .flatten()
    {
        if !contains_ci(&message.source, value) {
            return false;
        }
    }
    true
}

impl ImapClient for FakeClient {
    fn capabilities(&self) -> &HashSet<String> {
        &self.capabilities
    }

    fn selected(&self) -> Option<&SelectedMailbox> {
        self.selected.as_ref()
    }

    fn usable(&self) -> bool {
        self.usable && self.server.lock().epoch == self.epoch
    }

    fn take_events(&mut self) -> Vec<MailboxEvent> {
        std::mem::take(&mut self.events)
    }

    fn list(&mut self) -> BoxFuture<'_, ImapResult<Vec<MailboxInfo>>> {
        Box::pin(async move {
            self.enter(FakeOp::List, FakeCall::List).await?;
            Ok(self
                .server
                .lock()
                .folders
                .iter()
                .map(|f| MailboxInfo {
                    path: f.path.clone(),
                    flags: f.flags.clone(),
                    special_use: f.special_use.clone(),
                })
                .collect())
        })
    }

    fn select<'a>(&'a mut self, path: &'a str, read_only: bool) -> BoxFuture<'a, ImapResult<()>> {
        Box::pin(async move {
            self.enter(
                FakeOp::Select,
                FakeCall::Select {
                    path: path.to_owned(),
                    read_only,
                },
            )
            .await?;
            let mut state = self.server.lock();
            if let Some(hook) = state.after_select.take() {
                hook(path, &mut state);
                state.after_select = Some(hook);
            }
            let Some(folder) = state.folder(path) else {
                self.selected = None;
                return Err(ImapError::new(
                    format!("SELECT {path}"),
                    "Mailbox does not exist",
                ));
            };
            self.selected = Some(SelectedMailbox {
                path: path.to_owned(),
                uid_validity: folder.uid_validity.to_string(),
                read_only,
            });
            Ok(())
        })
    }

    fn status<'a>(&'a mut self, path: &'a str) -> BoxFuture<'a, ImapResult<FolderStatus>> {
        Box::pin(async move {
            self.enter(
                FakeOp::Status,
                FakeCall::Status {
                    path: path.to_owned(),
                },
            )
            .await?;
            let state = self.server.lock();
            let folder = state.folder(path).ok_or_else(|| {
                ImapError::new(format!("STATUS {path}"), "Mailbox does not exist")
            })?;
            Ok(FolderStatus {
                uid_next: Some(folder.uid_next),
                uid_validity: Some(folder.uid_validity),
            })
        })
    }

    fn uid_search<'a>(
        &'a mut self,
        criteria: &'a SearchCriteria,
    ) -> BoxFuture<'a, ImapResult<Option<Vec<u32>>>> {
        Box::pin(async move {
            let folder = self.folder_name();
            self.enter(
                FakeOp::Search,
                FakeCall::Search {
                    folder: folder.clone(),
                    criteria: criteria.clone(),
                },
            )
            .await?;
            let state = self.server.lock();
            if let Some(hook) = &state.search_override
                && let Some(result) = hook(&folder, criteria)
            {
                return result;
            }
            let Some(f) = state.folder(&folder) else {
                return Ok(None);
            };
            Ok(Some(
                f.messages
                    .iter()
                    .filter(|(_, m)| matches(m, criteria))
                    .map(|(uid, _)| *uid)
                    .collect(),
            ))
        })
    }

    fn uid_fetch<'a>(
        &'a mut self,
        uids: &'a UidSet,
        query: FetchQuery,
    ) -> BoxFuture<'a, ImapResult<Vec<FetchedMessage>>> {
        Box::pin(async move {
            let folder = self.folder_name();
            self.enter(
                FakeOp::Fetch,
                FakeCall::Fetch {
                    folder: folder.clone(),
                    uids: uids.clone(),
                    query,
                },
            )
            .await?;
            let state = self.server.lock();
            if let Some(hook) = &state.fetch_override
                && let Some(result) = hook(&folder, uids, &query)
            {
                return result;
            }
            let Some(f) = state.folder(&folder) else {
                return Ok(Vec::new());
            };
            let selected: Vec<(u32, &FakeMessage)> = match uids {
                UidSet::List(list) => list
                    .iter()
                    .filter_map(|uid| f.messages.get(uid).map(|m| (*uid, m)))
                    .collect(),
                UidSet::From(from) => {
                    let found: Vec<(u32, &FakeMessage)> = f
                        .messages
                        .range(*from..)
                        .map(|(uid, m)| (*uid, m))
                        .collect();
                    if found.is_empty() {
                        // `n:*` always includes the highest UID.
                        f.messages
                            .iter()
                            .next_back()
                            .map(|(u, m)| vec![(*u, m)])
                            .unwrap_or_default()
                    } else {
                        found
                    }
                }
            };
            Ok(selected
                .into_iter()
                .map(|(uid, m)| FetchedMessage {
                    uid,
                    source: query.source.map(|range| match range {
                        SourceRange::Full => m.source.clone(),
                        SourceRange::Prefix { max_length } => {
                            m.source.iter().take(max_length).copied().collect()
                        }
                    }),
                    internal_date_ms: if query.internal_date {
                        m.internal_date_ms
                    } else {
                        None
                    },
                    flags: query.flags.then(|| m.flags.clone()),
                    envelope_message_id: if query.envelope {
                        m.envelope_message_id.clone()
                    } else {
                        None
                    },
                    size: query.size.then_some(m.source.len() as u64),
                })
                .collect())
        })
    }

    fn uid_move<'a>(
        &'a mut self,
        uid: u32,
        destination: &'a str,
    ) -> BoxFuture<'a, ImapResult<Option<CopyUid>>> {
        Box::pin(async move {
            let folder = self.folder_name();
            self.enter(
                FakeOp::Move,
                FakeCall::Move {
                    folder: folder.clone(),
                    uid,
                    destination: destination.to_owned(),
                },
            )
            .await?;
            let mut state = self.server.lock();
            if let Some(hook) = &state.copy_override
                && let Some(result) = hook(FakeOp::Move)
            {
                return result;
            }
            let Some(message) = state
                .folder_mut(&folder)
                .and_then(|f| f.messages.remove(&uid))
            else {
                return Ok(None);
            };
            let Some(dest) = state.folder_mut(destination) else {
                return Ok(None);
            };
            let new_uid = dest.uid_next;
            dest.uid_next += 1;
            dest.messages.insert(new_uid, message);
            self.events.push(MailboxEvent::Expunge);
            Ok(Some(CopyUid {
                uid_validity: dest.uid_validity,
                uid_map: vec![(uid, new_uid)],
            }))
        })
    }

    fn uid_copy<'a>(
        &'a mut self,
        uid: u32,
        destination: &'a str,
    ) -> BoxFuture<'a, ImapResult<Option<CopyUid>>> {
        Box::pin(async move {
            let folder = self.folder_name();
            self.enter(
                FakeOp::Copy,
                FakeCall::Copy {
                    folder: folder.clone(),
                    uid,
                    destination: destination.to_owned(),
                },
            )
            .await?;
            let mut state = self.server.lock();
            if let Some(hook) = &state.copy_override
                && let Some(result) = hook(FakeOp::Copy)
            {
                return result;
            }
            let Some(message) = state
                .folder(&folder)
                .and_then(|f| f.messages.get(&uid).cloned())
            else {
                return Ok(None);
            };
            let Some(dest) = state.folder_mut(destination) else {
                return Ok(None);
            };
            let new_uid = dest.uid_next;
            dest.uid_next += 1;
            dest.messages.insert(new_uid, message);
            Ok(Some(CopyUid {
                uid_validity: dest.uid_validity,
                uid_map: vec![(uid, new_uid)],
            }))
        })
    }

    fn uid_store_add_flags<'a>(
        &'a mut self,
        uids: &'a [u32],
        flags: &'a [&'a str],
        silent: bool,
    ) -> BoxFuture<'a, ImapResult<bool>> {
        Box::pin(async move {
            let folder = self.folder_name();
            self.enter(
                FakeOp::Store,
                FakeCall::Store {
                    folder: folder.clone(),
                    uids: uids.to_vec(),
                    flags: flags.iter().map(|f| (*f).to_owned()).collect(),
                    silent,
                },
            )
            .await?;
            let mut state = self.server.lock();
            if let Some(result) = run_bool_override(&mut state, FakeOp::Store) {
                return result;
            }
            let Some(f) = state.folder_mut(&folder) else {
                return Ok(false);
            };
            let mut all = true;
            for uid in uids {
                match f.messages.get_mut(uid) {
                    Some(message) => {
                        for flag in flags {
                            if !message.flags.iter().any(|existing| existing == flag) {
                                message.flags.push((*flag).to_owned());
                            }
                        }
                    }
                    None => all = false,
                }
            }
            Ok(all)
        })
    }

    fn uid_expunge(&mut self, uid: u32) -> BoxFuture<'_, ImapResult<bool>> {
        Box::pin(async move {
            let folder = self.folder_name();
            self.enter(
                FakeOp::Expunge,
                FakeCall::Expunge {
                    folder: folder.clone(),
                    uid,
                },
            )
            .await?;
            if !self.capabilities.contains("UIDPLUS") {
                return Err(ImapError::new(
                    "UID EXPUNGE",
                    "Exact UID EXPUNGE unavailable",
                ));
            }
            let mut state = self.server.lock();
            if let Some(result) = run_bool_override(&mut state, FakeOp::Expunge) {
                return result;
            }
            let Some(f) = state.folder_mut(&folder) else {
                return Ok(false);
            };
            let deleted = f.messages.get(&uid).is_some_and(|m| {
                m.flags
                    .iter()
                    .any(|flag| flag.eq_ignore_ascii_case("\\Deleted"))
            });
            if !deleted {
                return Ok(false);
            }
            f.messages.remove(&uid);
            self.events.push(MailboxEvent::Expunge);
            Ok(true)
        })
    }

    fn append<'a>(
        &'a mut self,
        path: &'a str,
        content: &'a [u8],
        flags: &'a [&'a str],
        internal_date_ms: Option<i64>,
    ) -> BoxFuture<'a, ImapResult<bool>> {
        Box::pin(async move {
            self.enter(
                FakeOp::Append,
                FakeCall::Append {
                    path: path.to_owned(),
                    flags: flags.iter().map(|f| (*f).to_owned()).collect(),
                    internal_date_ms,
                },
            )
            .await?;
            let mut state = self.server.lock();
            if let Some(result) = run_bool_override(&mut state, FakeOp::Append) {
                return result;
            }
            let Some(f) = state.folder_mut(path) else {
                return Ok(false);
            };
            let uid = f.uid_next;
            f.uid_next += 1;
            let mut message = FakeMessage::new(content.to_vec());
            message.flags = flags.iter().map(|f| (*f).to_owned()).collect();
            message.internal_date_ms = internal_date_ms;
            f.messages.insert(uid, message);
            Ok(true)
        })
    }

    fn idle<'a>(
        &'a mut self,
        interrupt: &'a Notify,
        max: Duration,
    ) -> BoxFuture<'a, ImapResult<IdleEnd>> {
        Box::pin(async move {
            self.server.lock().calls.push(FakeCall::Idle);
            tokio::select! {
                () = interrupt.notified() => Ok(IdleEnd::Interrupted),
                () = tokio::time::sleep(max) => Ok(IdleEnd::TimedOut),
            }
        })
    }

    fn logout(&mut self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.server.lock().calls.push(FakeCall::Logout);
            self.usable = false;
        })
    }
}
