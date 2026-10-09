//! Parcel pipeline failures (`src/parcel-tracker/effect.ts`).

use omni_store::StoreError;

#[derive(Debug, thiserror::Error)]
pub enum ParcelError {
    /// The extraction model call failed (`transient`) or its output was unusable.
    #[error("Parcel extraction failed: {message}")]
    Extraction { message: String, transient: bool },
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
            ParcelError::Persistence { .. } => true,
        }
    }

    pub fn persistence(operation: &'static str) -> impl FnOnce(StoreError) -> ParcelError {
        move |source| ParcelError::Persistence { operation, source }
    }
}
