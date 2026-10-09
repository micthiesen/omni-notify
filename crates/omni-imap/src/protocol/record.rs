//! `SideEffectMode::Record`: an [`ImapClient`] decorator that forwards reads and
//! records (instead of sending) every mailbox mutation: MOVE, COPY, STORE,
//! UID EXPUNGE and APPEND. A recorded mutation fails the calling workflow with
//! a clear error, so no durable receipt ever claims a write that did not happen.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::BoxFuture;
use tokio::sync::Notify;

use super::{
    CopyUid, FetchQuery, FetchedMessage, FolderStatus, IdleEnd, ImapClient, ImapError, ImapResult,
    MailboxEvent, MailboxInfo, SearchCriteria, SelectedMailbox, UidSet,
};

/// A mutation that was recorded instead of sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordedImapWrite {
    Move {
        mailbox: String,
        uid: u32,
        destination: String,
    },
    Copy {
        mailbox: String,
        uid: u32,
        destination: String,
    },
    StoreFlags {
        mailbox: String,
        uids: Vec<u32>,
        flags: Vec<String>,
    },
    UidExpunge {
        mailbox: String,
        uid: u32,
    },
    Append {
        mailbox: String,
        bytes: usize,
        flags: Vec<String>,
    },
}

pub type RecordedWrites = Arc<Mutex<Vec<RecordedImapWrite>>>;

pub struct RecordingClient {
    inner: Box<dyn ImapClient>,
    recorded: RecordedWrites,
}

impl RecordingClient {
    pub fn new(inner: Box<dyn ImapClient>, recorded: RecordedWrites) -> Self {
        Self { inner, recorded }
    }

    fn record<T>(&self, operation: &str, write: RecordedImapWrite) -> ImapResult<T> {
        self.recorded
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(write);
        Err(ImapError::new(
            operation,
            "IMAP writes are recorded, not sent (SideEffectMode::Record)",
        ))
    }

    fn mailbox(&self) -> String {
        self.inner
            .selected()
            .map(|s| s.path.clone())
            .unwrap_or_default()
    }
}

impl ImapClient for RecordingClient {
    fn capabilities(&self) -> &HashSet<String> {
        self.inner.capabilities()
    }

    fn selected(&self) -> Option<&SelectedMailbox> {
        self.inner.selected()
    }

    fn usable(&self) -> bool {
        self.inner.usable()
    }

    fn take_events(&mut self) -> Vec<MailboxEvent> {
        self.inner.take_events()
    }

    fn list(&mut self) -> BoxFuture<'_, ImapResult<Vec<MailboxInfo>>> {
        self.inner.list()
    }

    fn select<'a>(&'a mut self, path: &'a str, read_only: bool) -> BoxFuture<'a, ImapResult<()>> {
        // Read-only selection is enough for every recorded mutation's reads.
        let _ = read_only;
        self.inner.select(path, true)
    }

    fn status<'a>(&'a mut self, path: &'a str) -> BoxFuture<'a, ImapResult<FolderStatus>> {
        self.inner.status(path)
    }

    fn uid_search<'a>(
        &'a mut self,
        criteria: &'a SearchCriteria,
    ) -> BoxFuture<'a, ImapResult<Option<Vec<u32>>>> {
        self.inner.uid_search(criteria)
    }

    fn uid_fetch<'a>(
        &'a mut self,
        uids: &'a UidSet,
        query: FetchQuery,
    ) -> BoxFuture<'a, ImapResult<Vec<FetchedMessage>>> {
        self.inner.uid_fetch(uids, query)
    }

    fn uid_move<'a>(
        &'a mut self,
        uid: u32,
        destination: &'a str,
    ) -> BoxFuture<'a, ImapResult<Option<CopyUid>>> {
        let write = RecordedImapWrite::Move {
            mailbox: self.mailbox(),
            uid,
            destination: destination.to_owned(),
        };
        let result = self.record("UID MOVE", write);
        Box::pin(async move { result })
    }

    fn uid_copy<'a>(
        &'a mut self,
        uid: u32,
        destination: &'a str,
    ) -> BoxFuture<'a, ImapResult<Option<CopyUid>>> {
        let write = RecordedImapWrite::Copy {
            mailbox: self.mailbox(),
            uid,
            destination: destination.to_owned(),
        };
        let result = self.record("UID COPY", write);
        Box::pin(async move { result })
    }

    fn uid_store_add_flags<'a>(
        &'a mut self,
        uids: &'a [u32],
        flags: &'a [&'a str],
        _silent: bool,
    ) -> BoxFuture<'a, ImapResult<bool>> {
        let write = RecordedImapWrite::StoreFlags {
            mailbox: self.mailbox(),
            uids: uids.to_vec(),
            flags: flags.iter().map(|f| (*f).to_owned()).collect(),
        };
        let result = self.record("UID STORE", write);
        Box::pin(async move { result })
    }

    fn uid_expunge(&mut self, uid: u32) -> BoxFuture<'_, ImapResult<bool>> {
        let write = RecordedImapWrite::UidExpunge {
            mailbox: self.mailbox(),
            uid,
        };
        let result = self.record("UID EXPUNGE", write);
        Box::pin(async move { result })
    }

    fn append<'a>(
        &'a mut self,
        path: &'a str,
        content: &'a [u8],
        flags: &'a [&'a str],
        _internal_date_ms: Option<i64>,
    ) -> BoxFuture<'a, ImapResult<bool>> {
        let write = RecordedImapWrite::Append {
            mailbox: path.to_owned(),
            bytes: content.len(),
            flags: flags.iter().map(|f| (*f).to_owned()).collect(),
        };
        let result = self.record("APPEND", write);
        Box::pin(async move { result })
    }

    fn idle<'a>(
        &'a mut self,
        interrupt: &'a Notify,
        max: Duration,
    ) -> BoxFuture<'a, ImapResult<IdleEnd>> {
        self.inner.idle(interrupt, max)
    }

    fn logout(&mut self) -> BoxFuture<'_, ()> {
        self.inner.logout()
    }
}
