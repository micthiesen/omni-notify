//! Stylist failures. Every expected rejection carries a stable code that
//! tools render as `[code] message`.

use omni_store::StoreError;

#[derive(Debug, thiserror::Error)]
pub enum StylistError {
    /// An expected rejection: `validation`, `not_found`, `revision_conflict`,
    /// `idempotency_key_reused`, `immutable`, `duplicate_receipt`,
    /// `duplicate_variant`, `basis_mismatch`, `post_hoc`, `regression_incomplete`,
    /// `namespace`, `integrity`, `too_large`, `unsupported_media`, `archive`.
    #[error("{message}")]
    Rejected { code: &'static str, message: String },
    #[error("store: {0}")]
    Store(#[from] StoreError),
    #[error("filesystem: {0}")]
    Io(String),
    #[error("stored data could not be decoded: {0}")]
    Corrupt(String),
}

impl StylistError {
    pub fn code(&self) -> &'static str {
        match self {
            StylistError::Rejected { code, .. } => code,
            StylistError::Store(_) => "store",
            StylistError::Io(_) => "io",
            StylistError::Corrupt(_) => "corrupt",
        }
    }

    /// `[code] message`, the tool-facing text.
    pub fn tool_text(&self) -> String {
        format!("[{}] {self}", self.code())
    }
}

/// An expected rejection with `code`.
pub fn reject(code: &'static str, message: impl Into<String>) -> StylistError {
    StylistError::Rejected {
        code,
        message: message.into(),
    }
}

pub fn invalid(message: impl Into<String>) -> StylistError {
    reject("validation", message)
}

pub fn not_found(message: impl Into<String>) -> StylistError {
    reject("not_found", message)
}

pub fn io(error: impl std::fmt::Display) -> StylistError {
    StylistError::Io(error.to_string())
}

pub fn sql(error: rusqlite::Error) -> StylistError {
    StylistError::Store(StoreError::Sqlite(error.to_string()))
}
