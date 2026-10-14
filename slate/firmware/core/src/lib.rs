//! Omni Slate device behaviour, with no dependency on any hardware.
//!
//! Everything the firmware contract (`docs/firmware.md`) specifies belongs here as plain,
//! deterministic Rust: the update cycle, the battery bands, and offline backoff. The crate
//! is sans-I/O: it will consume sampled inputs, network outcomes and a monotonic
//! [`time::Millis`], and return [`cycle::Action`]s for `../app` to apply. Nothing here
//! blocks, sleeps or touches a peripheral, so the whole contract is testable on a laptop.
//!
//! Status: contract types only. The state machine and its tests land with bring-up.

#![cfg_attr(not(test), no_std)]

pub mod battery;
pub mod cycle;
pub mod offline;
pub mod time;
