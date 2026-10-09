//! Cross-subsystem ports. Each port is set once during wiring (WP14) and read
//! through its accessor; a consumer must handle an unset port (the providing
//! subsystem is disabled or not yet ported).
//!
//! Payloads that are DTOs owned by another package's `omni-api` module cross
//! as `serde_json::Value` produced by serializing that DTO; the consumer
//! deserializes into the same `omni-api` type. This keeps ports stable while
//! WP04/05/08/11 define their DTOs.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use futures::future::BoxFuture;
use omni_core::email::{
    DownloadedAttachment, EmailAttachment, EmailHandler, EmailOrigin, FetchedEmail,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// A port call failure.
#[derive(Debug, thiserror::Error)]
pub enum PortError {
    #[error("{0} is unavailable")]
    Unavailable(&'static str),
    #[error("{message}")]
    Failed { message: String, transient: bool },
}

/// `EmailSearchOptions` (`src/email/types.ts`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailSearch {
    pub query: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub subject: Option<String>,
    pub unread: Option<bool>,
    /// IMAP internal date lower bound (inclusive, day precision), epoch ms.
    pub since_ms: Option<i64>,
    /// IMAP internal date upper bound (exclusive, day precision), epoch ms.
    pub before_ms: Option<i64>,
    pub folder: Option<EmailFolderScope>,
    pub limit: u32,
    /// Bypass the search and parsed-message caches.
    pub fresh: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EmailFolderScope {
    Inbox,
    Archive,
    Sent,
    All,
}

/// Transport capabilities reported by `email_health`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmailReaderHealth {
    /// Short transport label (`"IMAP"`).
    pub transport: String,
    pub search_available: bool,
    pub drafts_available: bool,
}

/// Implemented by WP01; used by WP02 (retry, reprocess, MCP email tools), WP03
/// (attachment download) and WP11.
pub trait EmailReader: Send + Sync {
    /// Re-fetches one email by stable id; `None` when it is gone.
    fn fetch_by_id<'a>(
        &'a self,
        id: &'a str,
        fresh: bool,
    ) -> BoxFuture<'a, Result<Option<FetchedEmail>, PortError>>;
    fn search<'a>(
        &'a self,
        q: &'a EmailSearch,
    ) -> BoxFuture<'a, Result<Vec<FetchedEmail>, PortError>>;
    fn health(&self) -> EmailReaderHealth;
    /// `downloadAttachmentEffect`: the attachment's bytes by its folder/UID
    /// handle; `None` when the message or part is no longer available.
    fn download_attachment<'a>(
        &'a self,
        attachment: &'a EmailAttachment,
    ) -> BoxFuture<'a, Result<Option<DownloadedAttachment>, PortError>>;
}

/// Implemented by WP01 (`omni_imap::transport::StoreArchiveEcho`, which needs only
/// the store); used by WP12 to suppress mailbox events caused by its own archive
/// moves. WP14 must set it whenever MCP events are enabled: the MCP package fails
/// boot without it rather than publishing duplicate `email.received` events.
pub trait ArchiveEcho: Send + Sync {
    fn is_archive_action_message<'a>(
        &'a self,
        message_id: &'a str,
        origin: Option<&'a EmailOrigin>,
    ) -> BoxFuture<'a, Result<bool, PortError>>;
}

/// Implemented by WP14 wiring; used by WP02 retry/reprocess to find a pipeline's handler.
pub trait EmailRetryHandlers: Send + Sync {
    fn handler(&self, pipeline: &str) -> Option<Arc<dyn EmailHandler>>;
}

/// `WorkspaceCalendarEventPayload`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarEventInput {
    pub title: String,
    pub start_date: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_time: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_time: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_zone: Option<String>,
    pub all_day: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reminder_minutes: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CalendarCreateOutcome {
    Created {
        event_uid: String,
    },
    /// HTTP 412: an event with this UID already exists.
    AlreadyExists,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalendarWriterStatus {
    pub configured: bool,
    pub provider: Option<String>,
}

/// Implemented by WP03; used by WP11 (UID `workspace-<actionId>@omni-notify`).
pub trait CalendarWriter: Send + Sync {
    fn create_event<'a>(
        &'a self,
        uid: &'a str,
        input: &'a CalendarEventInput,
    ) -> BoxFuture<'a, Result<CalendarCreateOutcome, PortError>>;
    fn status(&self) -> CalendarWriterStatus;
}

/// Implemented by WP04; used by WP05, WP12 and WP14. Values are serialized
/// `omni_api::streamers` DTOs.
pub trait LiveDirectory: Send + Sync {
    /// Configured streamers (`channels.json` order).
    fn streamers(&self) -> BoxFuture<'_, Result<Vec<Value>, PortError>>;
    /// Current aggregate statuses.
    fn statuses(&self) -> BoxFuture<'_, Result<Vec<Value>, PortError>>;
    /// `serializeStreamersForDisplay` (dashboard snapshot and `/api/streamers`).
    fn display(&self) -> BoxFuture<'_, Result<Vec<Value>, PortError>>;
    fn details<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<Option<Value>, PortError>>;
}

/// An aggregate live-state edge for intelligence hooks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveTransition {
    pub streamer_id: String,
    pub live: bool,
    pub at_ms: i64,
}

/// One streamer polled live this tick (TS `LiveObservation`): sent for the
/// went-live edge and every still-live poll, never for a streamer whose poll
/// failed or was not due (background tier).
#[derive(Clone, Debug, PartialEq)]
pub struct LiveObservation {
    /// A serialized `omni_api::streamers::LivestreamSummary`.
    pub streamer: Value,
    /// The new live status as a serialized `omni_api::streamers::StreamerStatusView`.
    pub status: Value,
    pub went_live: bool,
    pub title_changed: bool,
    pub at_ms: i64,
}

/// Implemented by WP05; used by WP04 (task hooks, routes) and WP12. Values are
/// serialized `omni_api::intelligence` DTOs.
pub trait LiveIntelligence: Send + Sync {
    /// `observeLive`: a streamer polled live this tick.
    fn observe_live<'a>(&'a self, observation: &'a LiveObservation) -> BoxFuture<'a, ()>;
    /// Runs after every live-check tick.
    fn after_tick(&self) -> BoxFuture<'_, ()>;
    /// Aggregate edges; `live: false` is `observeOffline`.
    fn on_transition<'a>(&'a self, transition: &'a LiveTransition) -> BoxFuture<'a, ()>;
    fn details<'a>(
        &'a self,
        id: &'a str,
        limit: usize,
    ) -> BoxFuture<'a, Result<Option<Value>, PortError>>;
    fn diagnostics(&self) -> BoxFuture<'_, Result<Value, PortError>>;
    fn record_feedback(&self, input: Value) -> BoxFuture<'_, Result<Value, PortError>>;
}

/// Implemented by WP08; used by WP14 (snapshot). Values are serialized
/// `omni_api::media::OnDeckItem`s.
pub trait OnDeckSource: Send + Sync {
    fn on_deck(&self) -> BoxFuture<'_, Result<Vec<Value>, PortError>>;
}

/// Implemented by WP11; used by WP12 (`briefings_list`). Values are serialized
/// `omni_api::briefings` history entries, newest first.
pub trait BriefingsReader: Send + Sync {
    fn histories(&self) -> BoxFuture<'_, Result<Vec<Value>, PortError>>;
}

/// A turn Omni just started on the Claude Code host (`noteTurnStarted` input).
#[derive(Clone, Debug, PartialEq)]
pub struct ClaudeTurnStarted {
    pub session_id: String,
    pub id: Option<String>,
    pub project: Option<String>,
    /// The session revision the turn started from.
    pub revision: f64,
}

/// Implemented by WP12's `claude.session.turn_finished` watcher; used by the
/// Claude session tools so a turn that ends before the next poll still
/// produces an event. Failures are the implementation's to log.
pub trait ClaudeSessionNotifier: Send + Sync {
    fn note_turn_started<'a>(&'a self, turn: &'a ClaudeTurnStarted) -> BoxFuture<'a, ()>;
}

/// The bounded commands of the Claude Code host's session client.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HostCommand {
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

impl HostCommand {
    pub fn as_str(self) -> &'static str {
        match self {
            HostCommand::Projects => "projects",
            HostCommand::List => "list",
            HostCommand::Status => "status",
            HostCommand::Read => "read",
            HostCommand::Result => "result",
            HostCommand::Wait => "wait",
            HostCommand::Start => "start",
            HostCommand::Send => "send",
            HostCommand::Stop => "stop",
        }
    }
}

/// `DeviceLinkStatus` of a configured link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostLinkStatus {
    pub online: bool,
    pub disabled: bool,
    /// The host's own name; never surfaced through MCP.
    pub host: Option<String>,
    pub last_seen_at: Option<String>,
    pub pending_jobs: usize,
}

/// A relay failure (`DeviceLinkError`).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{detail} ({code})")]
pub struct HostError {
    pub code: String,
    pub detail: String,
    pub retryable: bool,
}

impl HostError {
    pub fn new(code: impl Into<String>, detail: impl Into<String>, retryable: bool) -> Self {
        Self {
            code: code.into(),
            detail: detail.into(),
            retryable,
        }
    }
}

/// The Claude Code host link: status plus bounded command execution.
/// Implemented by WP12's device link (`omni_device_link::DeviceLinkService`, the
/// outbound long-poll relay); used by WP12's Claude session tools, activity
/// routes and session watcher.
pub trait ClaudeHost: Send + Sync {
    fn status(&self) -> HostLinkStatus;
    /// Runs one command and returns the client's `data` payload.
    fn execute(
        &self,
        command: HostCommand,
        args: Map<String, Value>,
        timeout: Duration,
    ) -> BoxFuture<'_, Result<Map<String, Value>, HostError>>;
}

/// Returned when a port is set twice.
#[derive(Debug, thiserror::Error)]
#[error("port {0} is already set")]
pub struct PortAlreadySet(pub &'static str);

macro_rules! ports {
    ($( $field:ident, $setter:ident : $trait:ident ),* $(,)?) => {
        #[derive(Default)]
        struct PortsInner {
            $( $field: OnceLock<Arc<dyn $trait>>, )*
        }

        /// All ports; cheap to clone, set once each during wiring.
        #[derive(Clone, Default)]
        pub struct Ports {
            inner: Arc<PortsInner>,
        }

        impl Ports {
            $(
                pub fn $field(&self) -> Option<Arc<dyn $trait>> {
                    self.inner.$field.get().cloned()
                }

                pub fn $setter(&self, port: Arc<dyn $trait>) -> Result<(), PortAlreadySet> {
                    self.inner
                        .$field
                        .set(port)
                        .map_err(|_| PortAlreadySet(stringify!($trait)))
                }
            )*
        }
    };
}

ports! {
    email_reader, set_email_reader: EmailReader,
    archive_echo, set_archive_echo: ArchiveEcho,
    email_retry_handlers, set_email_retry_handlers: EmailRetryHandlers,
    calendar_writer, set_calendar_writer: CalendarWriter,
    live_directory, set_live_directory: LiveDirectory,
    live_intelligence, set_live_intelligence: LiveIntelligence,
    on_deck_source, set_on_deck_source: OnDeckSource,
    briefings_reader, set_briefings_reader: BriefingsReader,
    claude_session_notifier, set_claude_session_notifier: ClaudeSessionNotifier,
    claude_host, set_claude_host: ClaudeHost,
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Deck;

    impl OnDeckSource for Deck {
        fn on_deck(&self) -> BoxFuture<'_, Result<Vec<Value>, PortError>> {
            Box::pin(async { Ok(vec![Value::Null]) })
        }
    }

    #[test]
    fn ports_are_set_once() {
        let ports = Ports::default();
        assert!(ports.on_deck_source().is_none());
        assert!(ports.set_on_deck_source(Arc::new(Deck)).is_ok());
        assert!(ports.clone().on_deck_source().is_some());
        assert!(ports.set_on_deck_source(Arc::new(Deck)).is_err());
    }
}
