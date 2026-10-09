//! `ObserverRepair`: Luna interprets Observer issue reports; code executes the
//! chosen scoped repair through Sonarr/Radarr and completes the issue only
//! after verification (`docs/observer-repair.md`).

pub mod agent;
pub mod arr;
pub mod persistence;
pub mod service;
pub mod task;
