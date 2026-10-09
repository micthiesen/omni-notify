//! Codex reset alerts from Reset Beacon. Predictions,
//! announcements, observed rollouts and reported landings stay distinct; a
//! landing needs completed history or measured banked-credit receipt.

pub mod history;
pub mod policy;
pub mod source;
pub mod task;

/// The shared delivery ledger in the unchanged `codex-reset-delivery`
/// namespace.
pub mod delivery {
    pub use crate::reset_alerts::delivery::{
        Codex, CodexResetDelivery as ResetDeliveryEntity, DeliveryError, ResetAlert,
    };
    /// The Codex ledger.
    pub type CodexLedger = crate::reset_alerts::ResetDeliveryLedger<Codex>;
}
