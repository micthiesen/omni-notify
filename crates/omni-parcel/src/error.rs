//! Parcel pipeline failures.

use omni_store::StoreError;

#[derive(Debug, thiserror::Error)]
pub enum ParcelError {
    /// The extraction model call failed (`transient`) or its output was unusable.
    #[error("Parcel extraction failed: {message}")]
    Extraction { message: String, transient: bool },
    /// The extraction request itself was rejected (see `omni_email::systemic`):
    /// the email waits for a new build instead of the retry schedule.
    #[error("Parcel extraction failed: {message}")]
    SystemicExtraction {
        message: String,
        signature: omni_email::systemic::Signature,
    },
    /// A durable read or write failed; always worth retrying.
    #[error("{operation} failed: {source}")]
    Persistence {
        operation: &'static str,
        #[source]
        source: StoreError,
    },
}

impl ParcelError {
    /// Network and 5xx-style failures enter the durable email retry queue.
    pub fn is_transient(&self) -> bool {
        match self {
            ParcelError::Extraction { transient, .. } => *transient,
            ParcelError::SystemicExtraction { .. } => false,
            ParcelError::Persistence { .. } => true,
        }
    }

    /// The signature of a systemic failure.
    pub fn systemic(&self) -> Option<&omni_email::systemic::Signature> {
        match self {
            ParcelError::SystemicExtraction { signature, .. } => Some(signature),
            ParcelError::Extraction { .. } | ParcelError::Persistence { .. } => None,
        }
    }

    pub fn persistence(operation: &'static str) -> impl FnOnce(StoreError) -> ParcelError {
        move |source| ParcelError::Persistence { operation, source }
    }
}
