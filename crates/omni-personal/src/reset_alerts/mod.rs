//! Shared reset alert infrastructure: bounded source
//! reads, durable delivery, presentation and the one-minute task.

pub mod delivery;
pub mod presentation;
pub mod source;
pub mod task;

pub use delivery::{
    ClaudeResetDelivery, CodexResetDelivery, DeliveryCounts, DeliveryError, NotifyError,
    PushoverNotifier, ResetAlert, ResetDeliveryLedger, ResetNotifier,
};
pub use source::ResetSourceError;
pub use task::{ResetAlertTask, ResetSnapshot, SnapshotSource};
