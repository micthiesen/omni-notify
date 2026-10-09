//! Manual reprocessing (`src/mcp/tools/email-reprocess.ts` and the reprocess
//! route): re-fetch the email and rerun its pipeline. A queued retry is
//! cleared only after the handler succeeds, so a failed reprocess keeps it.

use std::future::Future;

use omni_core::email::{EmailHandler, FetchedEmail, HandlerError};
use omni_runtime::Ports;
use omni_store::{Store, StoreError};

use crate::activity::{self, EmailActivityData};
use crate::retry;

const LOG: &str = "Main:Server";

/// `EmailReprocessError`.
#[derive(Debug, thiserror::Error)]
pub enum ReprocessError {
    #[error("{source}")]
    Handler {
        email_id: String,
        #[source]
        source: HandlerError,
    },
    #[error("{source}")]
    ClearRetry {
        email_id: String,
        #[source]
        source: StoreError,
    },
}

/// `handleEmailThenClearRetryEffect`: runs the handler, then (only on success)
/// clears the scheduled retry.
pub async fn handle_then_clear_retry<C, Fut>(
    handler: &dyn EmailHandler,
    email: &FetchedEmail,
    clear_retry: C,
) -> Result<(), ReprocessError>
where
    C: FnOnce() -> Fut,
    Fut: Future<Output = Result<(), StoreError>>,
{
    handler
        .handle(std::slice::from_ref(email))
        .await
        .map_err(|source| ReprocessError::Handler {
            email_id: email.id.clone(),
            source,
        })?;
    clear_retry()
        .await
        .map_err(|source| ReprocessError::ClearRetry {
            email_id: email.id.clone(),
            source,
        })
}

/// Why a reprocess could not run.
#[derive(Debug, thiserror::Error)]
pub enum ReprocessFailure {
    #[error("Unknown email activity: {0}")]
    UnknownActivity(String),
    #[error("Email pipelines are not active")]
    PipelinesInactive,
    #[error("Email pipeline is not active: {0}")]
    PipelineInactive(String),
    #[error("Email no longer exists in the mailbox")]
    EmailGone,
    #[error("{0}")]
    Fetch(String),
    #[error(transparent)]
    Reprocess(#[from] ReprocessError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// Re-fetches the activity's email and reruns its pipeline; returns the
/// refreshed activity row (or the original when the run left none).
pub async fn reprocess_activity(
    store: &Store,
    ports: &Ports,
    activity_id: &str,
) -> Result<EmailActivityData, ReprocessFailure> {
    let activity = activity::get(store, activity_id)
        .await?
        .ok_or_else(|| ReprocessFailure::UnknownActivity(activity_id.to_owned()))?;
    let reader = ports
        .email_reader()
        .ok_or(ReprocessFailure::PipelinesInactive)?;
    let handler = ports
        .email_retry_handlers()
        .and_then(|handlers| handlers.handler(activity.pipeline.as_str()))
        .ok_or_else(|| ReprocessFailure::PipelineInactive(activity.pipeline.to_string()))?;
    let email = reader
        .fetch_by_id(&activity.email_id, false)
        .await
        .map_err(|e| ReprocessFailure::Fetch(e.to_string()))?
        .ok_or(ReprocessFailure::EmailGone)?;
    tracing::info!(
        target: LOG,
        "Reprocessing \"{}\" through {}",
        activity.subject,
        activity.pipeline
    );
    let pipeline = activity.pipeline.as_str();
    handle_then_clear_retry(handler.as_ref(), &email, || async {
        retry::clear(store, pipeline, &activity.email_id)
            .await
            .map(|_| ())
    })
    .await?;
    Ok(activity::get(store, activity_id).await?.unwrap_or(activity))
}
