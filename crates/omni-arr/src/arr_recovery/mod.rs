//! `ArrRecovery`: settled Sonarr/Radarr import failures, observed unchanged for
//! 15 minutes, are imported, removed or replaced under deterministic guards
//! (`docs/arr-recovery.md`).

pub mod client;
pub mod filesystem;
pub mod llm;
pub mod nzbget;
pub mod persistence;
pub mod policy;
pub mod service;
pub mod task;
pub mod types;

pub use types::*;
