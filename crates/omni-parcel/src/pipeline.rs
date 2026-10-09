//! `ParcelTracker`: filter, extract,
//! validate carriers, submit with durable dedup, and record one activity row
//! per email whose outcome reflects per-delivery success.

use std::path::PathBuf;
use std::sync::Arc;

use futures::future::BoxFuture;
use omni_core::email::{EmailHandler, FetchedEmail, HandlerError};
use omni_email::activity::{
    self, AdmitTier, EmailActivityOutcome, EmailPipelineName, LlmCost, NewActivity,
    derive_items_outcome, sum_cost_cents,
};
use omni_email::triage::{EmailTriage, TriageEmail};
use omni_email::{activity_logs, retry};
use omni_store::{Store, StoreError};
use omni_tasks::RunLogs;
use tokio::sync::OnceCell;
use tokio_util::task::TaskTracker;

use crate::carriers::candidates::select_valid_candidates;
use crate::carriers::carrier_map::CarrierDirectory;
use crate::error::ParcelError;
use crate::extraction::{DeliveryExtractor, ExtractedDelivery, ExtractionEmail};
use crate::filter::{FilterDeps, FilterResult, filter_tracking_candidate};
use crate::log_file::{LogFile, LogFileMode, log_timestamp};
use crate::parcel_api::{ParcelSubmitter, SubmitParams, SubmitResult, should_try_next_candidate};
use crate::persistence::{self, DeliveryAttempt, SubmissionStatus, find_near_duplicate_tracking};

const LOG: &str = "Main:ParcelTracker";
pub const NAME: &str = "ParcelTracker";
const PIPELINE: EmailPipelineName = EmailPipelineName::ParcelTracker;

/// Everything the pipeline uses.
pub struct PipelineDeps {
    pub store: Store,
    pub run_logs: RunLogs,
    pub triage: EmailTriage,
    pub carriers: Arc<CarrierDirectory>,
    pub extractor: Arc<dyn DeliveryExtractor>,
    pub submitter: Arc<dyn ParcelSubmitter>,
    /// `EMAIL_SELF_ADDRESS`.
    pub self_address: Option<String>,
    /// `LOGS_PATH`: markdown run and rejection logs go under `parcel-tracker/`.
    pub logs_path: Option<PathBuf>,
    pub tz: jiff::tz::TimeZone,
    /// Submission sequences run to completion on this tracker.
    pub tracker: TaskTracker,
}

/// A short per-delivery result and whether it counts as success.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemResult {
    pub line: String,
    pub ok: bool,
}

impl ItemResult {
    fn ok(line: String) -> Self {
        Self { line, ok: true }
    }

    fn failed(line: String) -> Self {
        Self { line, ok: false }
    }
}

struct Candidate<'a> {
    email: &'a FetchedEmail,
    admit_reason: String,
    admit_tier: AdmitTier,
}

/// The parcel email handler.
pub struct DeliveryPipeline {
    deps: Arc<PipelineDeps>,
    rejection_log: OnceCell<Option<Arc<LogFile>>>,
}

impl DeliveryPipeline {
    pub fn new(deps: PipelineDeps) -> Self {
        Self {
            deps: Arc::new(deps),
            rejection_log: OnceCell::new(),
        }
    }

    async fn rejection_log(&self) -> Option<Arc<LogFile>> {
        let logs_path = self.deps.logs_path.clone()?;
        self.rejection_log
            .get_or_init(|| async move {
                let path = logs_path.join("parcel-tracker").join("rejections.md");
                match LogFile::make(path, LogFileMode::Append).await {
                    Ok(file) => Some(Arc::new(file)),
                    Err(error) => {
                        tracing::warn!(target: LOG, "Could not open the rejection log: {error}");
                        None
                    }
                }
            })
            .await
            .clone()
    }

    async fn run_log(&self) -> Option<LogFile> {
        let logs_path = self.deps.logs_path.as_ref()?;
        let stamp = log_timestamp(self.deps.store.clock().now_ms(), &self.deps.tz);
        let path = logs_path.join("parcel-tracker").join(format!("{stamp}.md"));
        match LogFile::make(path, LogFileMode::Overwrite).await {
            Ok(file) => Some(file),
            Err(error) => {
                tracing::warn!(target: LOG, "Could not open the run log: {error}");
                None
            }
        }
    }

    /// `handleEmailsEffect`. Filter-phase store failures fail the batch (the
    /// dispatcher then keeps its cursor); per-candidate failures are recorded
    /// as `error` activity and, when transient, enqueued for retry.
    pub async fn handle_emails(&self, emails: &[FetchedEmail]) -> Result<(), StoreError> {
        let rejection_log = self.rejection_log().await;
        let deps = &self.deps;
        let filter_deps = FilterDeps {
            store: &deps.store,
            triage: &deps.triage,
            carriers: &deps.carriers,
            self_address: deps.self_address.as_deref(),
        };
        let mut candidates = Vec::new();
        for email in emails {
            match filter_tracking_candidate(&filter_deps, &TriageEmail::from(email)).await? {
                FilterResult::Pass { reason, admit_tier } => {
                    tracing::info!(
                        target: LOG,
                        "Candidate ({reason}): \"{}\" from {}",
                        email.subject,
                        email.from
                    );
                    candidates.push(Candidate {
                        email,
                        admit_reason: reason,
                        admit_tier,
                    });
                }
                FilterResult::Skip { reason } => {
                    tracing::info!(
                        target: LOG,
                        "Skipped ({reason}): \"{}\" from {}",
                        email.subject,
                        email.from
                    );
                    activity::record(
                        &deps.store,
                        NewActivity {
                            detail: Some(reason),
                            cost_cents: deps.triage.triage_cost_cents(&email.id),
                            ..NewActivity::new(PIPELINE, email, EmailActivityOutcome::Filtered)
                        },
                    )
                    .await?;
                }
            }
        }
        for candidate in candidates {
            self.process_candidate(candidate, rejection_log.clone())
                .await;
        }
        Ok(())
    }

    async fn process_candidate(
        &self,
        candidate: Candidate<'_>,
        rejection_log: Option<Arc<LogFile>>,
    ) {
        let deps = &self.deps;
        let email = candidate.email;
        // Triage cost counts only when triage admitted the email; the shared
        // triage memoizes per email, so the calendar row may carry it too.
        let triage_cost = if candidate.admit_tier == AdmitTier::Triage {
            deps.triage.triage_cost_cents(&email.id)
        } else {
            LlmCost::None
        };
        let work = async {
            let run_log = self.run_log().await;
            let processed = self
                .process_email(email, run_log.as_ref(), rejection_log)
                .await;
            match processed {
                Ok((results, extraction_cost)) => {
                    let ok: Vec<bool> = results.iter().map(|r| r.ok).collect();
                    activity::record(
                        &deps.store,
                        NewActivity {
                            detail: results
                                .is_empty()
                                .then(|| "no tracking numbers found".to_owned()),
                            admit_reason: Some(candidate.admit_reason.clone()),
                            admit_tier: Some(candidate.admit_tier),
                            cost_cents: sum_cost_cents(&[triage_cost, extraction_cost]),
                            items: (!results.is_empty())
                                .then(|| results.into_iter().map(|r| r.line).collect()),
                            ..NewActivity::new(PIPELINE, email, derive_items_outcome(&ok))
                        },
                    )
                    .await
                    .map(|_| ())
                    .map_err(ParcelError::persistence("record parcel activity"))
                }
                Err(error) => {
                    tracing::error!(
                        target: LOG,
                        error = %error,
                        "Failed to process email \"{}\"",
                        email.subject
                    );
                    activity::record(
                        &deps.store,
                        NewActivity {
                            detail: Some(error.to_string()),
                            admit_reason: Some(candidate.admit_reason.clone()),
                            admit_tier: Some(candidate.admit_tier),
                            cost_cents: sum_cost_cents(&[triage_cost]),
                            ..NewActivity::new(PIPELINE, email, EmailActivityOutcome::Error)
                        },
                    )
                    .await
                    .map_err(ParcelError::persistence("record parcel activity"))?;
                    if error.is_transient() {
                        retry::enqueue(&deps.store, NAME, &email.id, &error.to_string())
                            .await
                            .map_err(ParcelError::persistence("enqueue parcel email retry"))?;
                    }
                    Ok(())
                }
            }
        };
        let activity_id = activity::activity_id(PIPELINE, &email.id);
        let outcome =
            activity_logs::with_capture(&deps.store, &deps.run_logs, &activity_id, NAME, work)
                .await;
        if let Err(error) = outcome {
            tracing::error!(
                target: LOG,
                error = %error,
                "Could not record parcel outcome for \"{}\"",
                email.subject
            );
        }
    }

    /// Per-delivery results plus the extraction call's cost.
    async fn process_email(
        &self,
        email: &FetchedEmail,
        run_log: Option<&LogFile>,
        rejection_log: Option<Arc<LogFile>>,
    ) -> Result<(Vec<ItemResult>, LlmCost), ParcelError> {
        tracing::info!(
            target: LOG,
            "Extracting from: \"{}\" (from: {})",
            email.subject,
            email.from
        );
        let input = ExtractionEmail {
            subject: email.subject.clone(),
            from: email.from.clone(),
            text_body: email.text_body.clone(),
            links: email.links.clone(),
        };
        let extracted = self.deps.extractor.extract(&input, run_log).await?;
        if extracted.deliveries.is_empty() {
            tracing::info!(target: LOG, "No tracking numbers found in \"{}\"", email.subject);
            return Ok((Vec::new(), extracted.cost));
        }
        tracing::info!(
            target: LOG,
            "Found {} delivery(ies) in \"{}\"",
            extracted.deliveries.len(),
            email.subject
        );
        let mut results = Vec::with_capacity(extracted.deliveries.len());
        for delivery in extracted.deliveries {
            // Reservation, submission and confirmation run to completion even if
            // the caller is dropped, so a split sequence never strands a submission.
            let deps = self.deps.clone();
            let email_id = email.id.clone();
            let log = rejection_log.clone();
            let result = omni_core::spawn::must_complete(&self.deps.tracker, async move {
                process_delivery(&deps, delivery, &email_id, log.as_deref()).await
            })
            .await?;
            results.push(result);
        }
        Ok((results, extracted.cost))
    }
}

/// One extracted delivery: dedup, carrier validation, ranked submission.
async fn process_delivery(
    deps: &PipelineDeps,
    delivery: ExtractedDelivery,
    email_id: &str,
    rejection_log: Option<&LogFile>,
) -> Result<ItemResult, ParcelError> {
    let store = &deps.store;
    let tracking_number = delivery.tracking_number.as_str();
    let prior = persistence::get(store, tracking_number)
        .await
        .map_err(ParcelError::persistence("read delivery reservation"))?;

    // Dedup reads persistence live so within-batch duplicates are caught.
    if persistence::has_submitted(store, tracking_number)
        .await
        .map_err(ParcelError::persistence("check submitted delivery"))?
    {
        tracing::info!(target: LOG, "Duplicate tracking number: {tracking_number} (skipping)");
        return Ok(ItemResult::ok(format!(
            "{tracking_number}: already submitted"
        )));
    }

    let known = persistence::all_tracking_numbers(store)
        .await
        .map_err(ParcelError::persistence("list submitted deliveries"))?;
    if let Some(near) =
        find_near_duplicate_tracking(tracking_number, known.iter().map(String::as_str))
    {
        tracing::info!(
            target: LOG,
            "Near-duplicate tracking number: {tracking_number} matches known {near} (skipping)"
        );
        return Ok(ItemResult::ok(format!(
            "{tracking_number}: near-duplicate of {near}, skipped"
        )));
    }

    let Some(valid_codes) = deps.carriers.valid_codes().await else {
        tracing::warn!(
            target: LOG,
            "Carrier list unavailable, cannot validate candidates for {tracking_number}"
        );
        return Ok(ItemResult::failed(format!(
            "{tracking_number}: carrier list unavailable"
        )));
    };
    let selection = select_valid_candidates(&delivery.carrier_candidates, &valid_codes);
    // A replayed pending reservation tries its reserved carrier first.
    let candidates: Vec<String> = match prior.as_ref() {
        Some(prior)
            if prior.status == Some(SubmissionStatus::Pending)
                && valid_codes.contains(&prior.carrier_code) =>
        {
            std::iter::once(prior.carrier_code.clone())
                .chain(
                    selection
                        .valid
                        .iter()
                        .filter(|code| **code != prior.carrier_code)
                        .cloned(),
                )
                .collect()
        }
        _ => selection.valid.clone(),
    };
    if !selection.invalid.is_empty() {
        tracing::warn!(
            target: LOG,
            "Dropped invalid carrier candidate(s) [{}] for {tracking_number}",
            selection.invalid.join(", ")
        );
    }
    if candidates.is_empty() {
        tracing::warn!(target: LOG, "No valid carrier candidates for {tracking_number}, skipping");
        return Ok(ItemResult::failed(format!(
            "{tracking_number}: no valid carrier candidates"
        )));
    }
    tracing::info!(
        target: LOG,
        "Carrier candidates for {tracking_number}: [{}]",
        candidates.join(", ")
    );

    // Ranked attempts; dedup is recorded only on a terminal outcome, so a
    // carrier-shaped rejection never blocks the fallback candidates.
    for (index, carrier_code) in candidates.iter().enumerate() {
        let label = format!("{tracking_number} ({carrier_code})");
        let attempt_label = format!("{}/{}", index + 1, candidates.len());
        let attempt = DeliveryAttempt {
            tracking_number: tracking_number.to_owned(),
            carrier_code: carrier_code.clone(),
            description: delivery.description.clone(),
            submitted_at: store.clock().now_ms(),
            email_id: email_id.to_owned(),
        };
        persistence::reserve(store, attempt.clone())
            .await
            .map_err(ParcelError::persistence("reserve Parcel submission"))?;

        let result = deps
            .submitter
            .submit(
                &SubmitParams {
                    tracking_number: tracking_number.to_owned(),
                    carrier_code: carrier_code.clone(),
                    description: delivery.description.clone(),
                },
                rejection_log,
            )
            .await;

        match result {
            SubmitResult::Success => {
                if index > 0 {
                    tracing::info!(
                        target: LOG,
                        "Fallback candidate \"{carrier_code}\" succeeded for {tracking_number} (attempt {attempt_label})"
                    );
                }
                let confirmed = persistence::get(store, tracking_number)
                    .await
                    .map_err(ParcelError::persistence("read delivery attempt"))?;
                persistence::record(
                    store,
                    attempt,
                    SubmissionStatus::Submitted,
                    confirmed.and_then(|row| row.attempts),
                )
                .await
                .map_err(ParcelError::persistence("confirm Parcel submission"))?;
                return Ok(ItemResult::ok(format!("{label}: submitted")));
            }
            SubmitResult::Error => {
                // Transient: keep the remaining candidates and let a retry pass replay.
                tracing::warn!(target: LOG, "Failed to submit {label}, will retry later");
                retry::enqueue(
                    store,
                    NAME,
                    email_id,
                    &format!("Parcel submission network/5xx for {tracking_number}"),
                )
                .await
                .map_err(ParcelError::persistence("enqueue parcel email retry"))?;
                return Ok(ItemResult::failed(format!(
                    "{label}: submission failed, will retry"
                )));
            }
            SubmitResult::Rejected { status } => {
                if should_try_next_candidate(result)
                    && let Some(next) = candidates.get(index + 1)
                {
                    tracing::warn!(
                        target: LOG,
                        "Parcel rejected {label} with {status} (attempt {attempt_label}), trying next candidate \"{next}\""
                    );
                    continue;
                }
                tracing::warn!(
                    target: LOG,
                    "Parcel rejected {label} with {status} (attempt {attempt_label}), recording to prevent retry"
                );
                let rejected = persistence::get(store, tracking_number)
                    .await
                    .map_err(ParcelError::persistence("read delivery attempt"))?;
                persistence::record(
                    store,
                    attempt,
                    SubmissionStatus::Rejected,
                    rejected.and_then(|row| row.attempts),
                )
                .await
                .map_err(ParcelError::persistence("confirm Parcel rejection"))?;
                return Ok(ItemResult::failed(format!(
                    "{label}: rejected by Parcel ({status})"
                )));
            }
        }
    }
    // Every iteration above returns or continues to a next candidate.
    Ok(ItemResult::failed(format!(
        "{tracking_number}: no submission attempted"
    )))
}

impl EmailHandler for DeliveryPipeline {
    fn name(&self) -> &'static str {
        NAME
    }

    fn handle<'a>(&'a self, emails: &'a [FetchedEmail]) -> BoxFuture<'a, Result<(), HandlerError>> {
        Box::pin(async move {
            self.handle_emails(emails)
                .await
                .map_err(|error| HandlerError::transient(error.to_string(), Some(Box::new(error))))
        })
    }
}
