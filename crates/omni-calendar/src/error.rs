//! Domain errors.

use omni_store::StoreError;

/// A CalDAV request failed at the transport level, or a response could not be
/// used. HTTP-level failures of writes are reported as [`crate::caldav::WriteOutcome`]
/// values instead.
#[derive(Debug, thiserror::Error)]
#[error("{operation} failed: {cause}")]
pub struct CaldavError {
    pub operation: String,
    pub cause: String,
    pub transient: bool,
    pub status: Option<u16>,
}

impl CaldavError {
    pub fn new(operation: impl Into<String>, cause: impl Into<String>, transient: bool) -> Self {
        Self {
            operation: operation.into(),
            cause: cause.into(),
            transient,
            status: None,
        }
    }

    pub(crate) fn with_status(mut self, status: u16) -> Self {
        self.status = Some(status);
        self
    }
}

/// The extraction model call failed or produced unusable output.
#[derive(Debug, thiserror::Error)]
#[error("Calendar extraction failed: {cause}")]
pub struct CalendarExtractionError {
    pub cause: String,
    pub transient: bool,
    /// Set when the request itself was rejected (see `omni_email::systemic`):
    /// the email waits for a new build instead of the retry schedule.
    pub systemic: Option<omni_email::systemic::Signature>,
}

impl CalendarExtractionError {
    /// Systemic failures wait for a new build; transient and content failures
    /// use the retry queue.
    pub fn from_ai(error: &omni_ai::AiError) -> Self {
        let systemic = match omni_email::systemic::classify_ai_error(error) {
            omni_email::systemic::FailureClass::Systemic(signature) => Some(signature),
            omni_email::systemic::FailureClass::Transient
            | omni_email::systemic::FailureClass::Content => None,
        };
        Self {
            cause: error.to_string(),
            transient: systemic.is_none(),
            systemic,
        }
    }
}

/// A tracked-event store operation failed.
#[derive(Debug, thiserror::Error)]
#[error("{operation} failed: {source}")]
pub struct CalendarPersistenceError {
    pub operation: &'static str,
    #[source]
    pub source: StoreError,
}

impl CalendarPersistenceError {
    pub(crate) fn wrap(operation: &'static str) -> impl FnOnce(StoreError) -> Self {
        move |source| Self { operation, source }
    }
}
