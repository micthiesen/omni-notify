//! Email pipeline and outcome labels.

use omni_api::email::{EmailActivityOutcome, EmailPipelineName};

/// `PIPELINE_LABELS`, in declaration order.
pub const PIPELINES: [EmailPipelineName; 2] = [
    EmailPipelineName::ParcelTracker,
    EmailPipelineName::CalendarEvents,
];

pub fn pipeline_label(pipeline: EmailPipelineName) -> &'static str {
    match pipeline {
        EmailPipelineName::ParcelTracker => "Parcels",
        EmailPipelineName::CalendarEvents => "Calendar",
    }
}

pub fn outcome_label(outcome: EmailActivityOutcome) -> &'static str {
    match outcome {
        EmailActivityOutcome::Filtered => "Filtered",
        EmailActivityOutcome::Skipped => "Skipped",
        EmailActivityOutcome::NoMatches => "No Matches",
        EmailActivityOutcome::Processed => "Processed",
        EmailActivityOutcome::Partial => "Partial",
        EmailActivityOutcome::Failed => "Failed",
        EmailActivityOutcome::Error => "Error",
    }
}
