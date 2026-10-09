//! Workspace errors.

use omni_core::BoxError;

/// Workspace failures. Each displays as its user-facing message.
#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    /// Rejected input or model output.
    #[error("{message}")]
    Validation {
        message: String,
        #[source]
        source: Option<BoxError>,
    },
    /// `"<operation> failed: <cause>"`.
    #[error("{operation} failed: {source}")]
    Operation {
        operation: &'static str,
        #[source]
        source: BoxError,
    },
    /// An approval or rejection that cannot proceed.
    #[error("{message}")]
    Action {
        action_id: String,
        message: String,
        #[source]
        source: Option<BoxError>,
    },
}

impl WorkspaceError {
    pub fn validation(message: impl Into<String>) -> Self {
        WorkspaceError::Validation {
            message: message.into(),
            source: None,
        }
    }

    pub fn validation_with(message: impl Into<String>, source: impl Into<BoxError>) -> Self {
        WorkspaceError::Validation {
            message: message.into(),
            source: Some(source.into()),
        }
    }

    pub fn action(action_id: &str, message: impl Into<String>) -> Self {
        WorkspaceError::Action {
            action_id: action_id.to_owned(),
            message: message.into(),
            source: None,
        }
    }

    pub fn operation(operation: &'static str, source: impl Into<BoxError>) -> Self {
        WorkspaceError::Operation {
            operation,
            source: source.into(),
        }
    }
}

/// `workspaceRepositoryEffect(operation, ...)`: wraps a repository failure.
pub fn op<E: Into<BoxError>>(operation: &'static str) -> impl FnOnce(E) -> WorkspaceError {
    move |source| WorkspaceError::operation(operation, source)
}
