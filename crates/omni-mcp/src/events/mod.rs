//! MCP Events (`src/mcp/events/**`, `docs/mcp-events.md`): the catalog, the
//! shared durable outbox, webhook signing and delivery, Executor delegated
//! authorization, the `events/*` methods and the Claude session watcher.

pub mod catalog;
pub mod claude_sessions;
pub mod crypto;
pub mod executor_auth;
pub mod persistence;
pub mod rpc;
pub mod service;
pub mod webhook;
