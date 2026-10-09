//! Error helpers and the edge-adapter errors of `src/effect/errors.ts`.

use crate::BoxError;

/// A failure raised while adapting a foreign (async) API at an infrastructure
/// edge: TS `IntegrationError`, produced by `fromPromise`. Displays as
/// `"<operation> failed: <cause>"`, which is what TS persists as run errors.
#[derive(Debug, thiserror::Error)]
#[error("{operation} failed: {source}")]
pub struct IntegrationError {
    pub operation: String,
    #[source]
    pub source: BoxError,
}

impl IntegrationError {
    pub fn new(operation: impl Into<String>, source: impl Into<BoxError>) -> Self {
        Self {
            operation: operation.into(),
            source: source.into(),
        }
    }
}

/// A failure raised while adapting synchronous persistence code: TS
/// `PersistenceError`, produced by `fromSync`. Same message shape.
#[derive(Debug, thiserror::Error)]
#[error("{operation} failed: {source}")]
pub struct PersistenceError {
    pub operation: String,
    #[source]
    pub source: BoxError,
}

impl PersistenceError {
    pub fn new(operation: impl Into<String>, source: impl Into<BoxError>) -> Self {
        Self {
            operation: operation.into(),
            source: source.into(),
        }
    }
}

/// The message of the innermost cause in `e`'s source chain (the `McpToolError`
/// rule from `src/mcp/tool.ts`): wrappers are skipped so callers see the leaf
/// failure.
pub fn chain_message(e: &dyn std::error::Error) -> String {
    let mut current = e;
    while let Some(next) = current.source() {
        current = next;
    }
    current.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, thiserror::Error)]
    #[error("outer")]
    struct Outer(#[source] Inner);

    #[derive(Debug, thiserror::Error)]
    #[error("inner detail")]
    struct Inner;

    #[test]
    fn returns_innermost_message() {
        assert_eq!(chain_message(&Outer(Inner)), "inner detail");
        assert_eq!(chain_message(&Inner), "inner detail");
    }
}
