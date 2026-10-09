//! The IMAP client seam. [`ImapClient`] covers what the transport needs
//! (mailbox selection, STATUS, LIST with special-use, UID
//! SEARCH/FETCH/MOVE/COPY/STORE/EXPUNGE, APPEND, IDLE). The production
//! implementation is [`raw::RawClient`], a thin client over `imap-proto`;
//! async-imap 0.11.3 discards COPYUID response codes (its `uid_mv`/`uid_copy`
//! return `()`), which the exact archive workflow needs. Tests implement the
//! trait with the in-memory server from `crate::fake`.

use std::collections::HashSet;
use std::time::Duration;

use futures::future::BoxFuture;
use tokio::sync::Notify;

pub mod connect;
pub mod raw;
pub mod record;
pub(crate) mod special_use;
mod special_use_names;
pub(crate) mod utf7;

/// A failed IMAP operation: `"<operation> failed: <cause>"` like
/// An IMAP operation failure. The source chain ends at the leaf detail, which is
/// what MCP tool errors surface (`toolErrorMessage` takes the innermost cause).
#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
#[error("{operation} failed: {cause}")]
pub struct ImapError {
    pub operation: String,
    #[source]
    pub cause: Box<ImapCause>,
}

/// The cause of an [`ImapError`]: a leaf detail or a wrapped inner failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImapCause {
    Leaf(String),
    Nested(ImapError),
}

impl std::fmt::Display for ImapCause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Leaf(detail) => f.write_str(detail),
            Self::Nested(inner) => write!(f, "{inner}"),
        }
    }
}

impl std::error::Error for ImapCause {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Leaf(_) => None,
            Self::Nested(inner) => Some(inner.cause.as_ref()),
        }
    }
}

impl ImapError {
    pub fn new(operation: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            operation: operation.into(),
            cause: Box::new(ImapCause::Leaf(detail.into())),
        }
    }

    /// Wraps an inner failure under another operation name.
    pub fn wrap(operation: impl Into<String>, inner: ImapError) -> Self {
        Self {
            operation: operation.into(),
            cause: Box::new(ImapCause::Nested(inner)),
        }
    }

    /// The innermost detail (what an MCP tool error reports).
    pub fn leaf(&self) -> String {
        match self.cause.as_ref() {
            ImapCause::Leaf(detail) => detail.clone(),
            ImapCause::Nested(inner) => inner.leaf(),
        }
    }
}

/// One LIST entry after special-use resolution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MailboxInfo {
    pub path: String,
    pub flags: Vec<String>,
    /// `\Sent`, `\Drafts`, `\Archive`, `\Junk`, `\Trash`, `\All`, `\Flagged`, `\Inbox`.
    pub special_use: Option<String>,
}

impl MailboxInfo {
    pub fn has_flag(&self, flag: &str) -> bool {
        self.flags.iter().any(|f| f.eq_ignore_ascii_case(flag))
    }

    pub fn special_use_is(&self, role: &str) -> bool {
        self.special_use
            .as_deref()
            .is_some_and(|s| s.eq_ignore_ascii_case(role))
    }
}

/// The currently selected mailbox.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectedMailbox {
    pub path: String,
    /// UIDVALIDITY as a decimal string.
    pub uid_validity: String,
    pub read_only: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FolderStatus {
    pub uid_next: Option<u32>,
    pub uid_validity: Option<u32>,
}

/// UID SEARCH criteria (the imapflow query object subset in use). An empty
/// criteria set searches `ALL`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchCriteria {
    pub text: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub subject: Option<String>,
    pub seen: Option<bool>,
    /// INTERNALDATE lower bound (day precision, UTC date of the instant).
    pub since_ms: Option<i64>,
    /// INTERNALDATE upper bound; a non-midnight instant includes its day.
    pub before_ms: Option<i64>,
    /// `HEADER <name> <value>` (substring match on the server).
    pub header: Option<(String, String)>,
}

impl SearchCriteria {
    pub fn message_id(message_id: &str) -> Self {
        Self {
            header: Some(("Message-ID".to_owned(), message_id.to_owned())),
            ..Self::default()
        }
    }
}

/// Which UIDs a FETCH covers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UidSet {
    List(Vec<u32>),
    /// `<uid>:*`
    From(u32),
}

/// What a FETCH returns. The message source is always read with `BODY.PEEK[]`
/// so reads never set `\Seen`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FetchQuery {
    pub source: Option<SourceRange>,
    pub internal_date: bool,
    pub flags: bool,
    pub envelope: bool,
    pub size: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceRange {
    Full,
    /// `BODY.PEEK[]<0.max_length>`
    Prefix {
        max_length: usize,
    },
}

impl FetchQuery {
    pub const UID_ONLY: Self = Self {
        source: None,
        internal_date: false,
        flags: false,
        envelope: false,
        size: false,
    };

    pub fn source_and_date() -> Self {
        Self {
            source: Some(SourceRange::Full),
            internal_date: true,
            ..Self::default()
        }
    }

    pub fn envelope() -> Self {
        Self {
            envelope: true,
            ..Self::default()
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FetchedMessage {
    pub uid: u32,
    pub source: Option<Vec<u8>>,
    pub internal_date_ms: Option<i64>,
    pub flags: Option<Vec<String>>,
    /// ENVELOPE Message-ID, trimmed.
    pub envelope_message_id: Option<String>,
    pub size: Option<u64>,
}

/// COPYUID / MOVE response mapping.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CopyUid {
    pub uid_validity: u32,
    pub uid_map: Vec<(u32, u32)>,
}

impl CopyUid {
    pub fn destination_of(&self, source: u32) -> Option<u32> {
        self.uid_map
            .iter()
            .find(|(s, _)| *s == source)
            .map(|(_, d)| *d)
    }
}

/// Unsolicited mailbox changes (imapflow `exists`, `flags`, `expunge` events).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MailboxEvent {
    Exists,
    Flags,
    Expunge,
}

/// Why an IDLE ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdleEnd {
    Interrupted,
    TimedOut,
    ServerEvent,
}

pub type ImapResult<T> = Result<T, ImapError>;

/// An authenticated IMAP session. "Lock" semantics are exclusive `&mut`
/// access; `select` replaces imapflow's `getMailboxLock`.
pub trait ImapClient: Send {
    /// Post-authentication capabilities, uppercased.
    fn capabilities(&self) -> &HashSet<String>;
    fn has_capability(&self, name: &str) -> bool {
        self.capabilities().contains(&name.to_ascii_uppercase())
    }
    fn selected(&self) -> Option<&SelectedMailbox>;
    /// False once the connection failed or closed.
    fn usable(&self) -> bool;
    /// Events observed since the last call.
    fn take_events(&mut self) -> Vec<MailboxEvent>;

    fn list(&mut self) -> BoxFuture<'_, ImapResult<Vec<MailboxInfo>>>;
    /// SELECT (`read_only = false`) or EXAMINE; a no-op when already selected in that mode.
    fn select<'a>(&'a mut self, path: &'a str, read_only: bool) -> BoxFuture<'a, ImapResult<()>>;
    fn status<'a>(&'a mut self, path: &'a str) -> BoxFuture<'a, ImapResult<FolderStatus>>;
    /// `None` when the server did not confirm the search (imapflow `false`).
    fn uid_search<'a>(
        &'a mut self,
        criteria: &'a SearchCriteria,
    ) -> BoxFuture<'a, ImapResult<Option<Vec<u32>>>>;
    fn uid_fetch<'a>(
        &'a mut self,
        uids: &'a UidSet,
        query: FetchQuery,
    ) -> BoxFuture<'a, ImapResult<Vec<FetchedMessage>>>;
    /// `None` when the server did not confirm the MOVE.
    fn uid_move<'a>(
        &'a mut self,
        uid: u32,
        destination: &'a str,
    ) -> BoxFuture<'a, ImapResult<Option<CopyUid>>>;
    fn uid_copy<'a>(
        &'a mut self,
        uid: u32,
        destination: &'a str,
    ) -> BoxFuture<'a, ImapResult<Option<CopyUid>>>;
    /// `UID STORE <uids> +FLAGS[.SILENT] (<flags>)`; false when not confirmed.
    fn uid_store_add_flags<'a>(
        &'a mut self,
        uids: &'a [u32],
        flags: &'a [&'a str],
        silent: bool,
    ) -> BoxFuture<'a, ImapResult<bool>>;
    /// `UID EXPUNGE <uid>` for one UID; never a mailbox-wide EXPUNGE.
    fn uid_expunge(&mut self, uid: u32) -> BoxFuture<'_, ImapResult<bool>>;
    /// APPEND with flags and an optional INTERNALDATE; false when not confirmed.
    fn append<'a>(
        &'a mut self,
        path: &'a str,
        content: &'a [u8],
        flags: &'a [&'a str],
        internal_date_ms: Option<i64>,
    ) -> BoxFuture<'a, ImapResult<bool>>;
    /// IDLE on the selected mailbox until `interrupt` fires, `max` elapses or
    /// the server reports a change.
    fn idle<'a>(
        &'a mut self,
        interrupt: &'a Notify,
        max: Duration,
    ) -> BoxFuture<'a, ImapResult<IdleEnd>>;
    fn logout(&mut self) -> BoxFuture<'_, ()>;
}

/// The single message or `None` when the UID is gone.
pub async fn fetch_one(
    client: &mut dyn ImapClient,
    uid: u32,
    query: FetchQuery,
) -> ImapResult<Option<FetchedMessage>> {
    let set = UidSet::List(vec![uid]);
    let mut found = client.uid_fetch(&set, query).await?;
    Ok(found
        .iter()
        .position(|m| m.uid == uid)
        .map(|i| found.swap_remove(i)))
}

/// Selected UIDVALIDITY as a string (`String(client.mailbox.uidValidity)`).
pub fn selected_validity(client: &dyn ImapClient) -> Option<String> {
    client.selected().map(|s| s.uid_validity.clone())
}
