//! The incoming mail transport seam, implemented by `omni-imap` and driven by
//! `omni-email`'s dispatcher.

use crate::BoxFuture;
use crate::email::FetchedEmail;

/// New mail since the persisted cursor plus the commit that advances it.
///
/// The dispatcher calls `commit` only after fan-out, so a crash mid-dispatch
/// re-delivers instead of dropping.
pub struct EmailPoll {
    pub emails: Vec<FetchedEmail>,
    pub commit: Box<dyn FnOnce() -> BoxFuture<'static, Result<(), PollError>> + Send>,
}

impl std::fmt::Debug for EmailPoll {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmailPoll")
            .field("emails", &self.emails.len())
            .finish_non_exhaustive()
    }
}

pub trait MailSource: Send + Sync {
    fn poll(&self) -> BoxFuture<'_, Result<EmailPoll, PollError>>;
    /// Fires (possibly spuriously) on IDLE EXISTS, reconnects and the 5-minute sweep.
    fn mail_events(&self) -> tokio::sync::broadcast::Receiver<()>;
    fn start(&self) -> BoxFuture<'_, Result<(), PollError>>;
    fn stop(&self) -> BoxFuture<'_, ()>;
}

#[derive(Debug, thiserror::Error)]
pub enum PollError {
    #[error("mail transport: {message}")]
    Transport { message: String, transient: bool },
    #[error("cursor commit failed: {0}")]
    Commit(String),
}
