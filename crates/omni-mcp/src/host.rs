//! The Claude Code host seam.
//!
//! Claude session tools, the activity routes and the session watcher reach the
//! host only through the `omni_runtime::ports::ClaudeHost` port, which
//! `omni_device_link::DeviceLink::from_context` sets to the outbound long-poll
//! relay.

pub use omni_runtime::ports::{ClaudeHost, HostCommand, HostError, HostLinkStatus};
