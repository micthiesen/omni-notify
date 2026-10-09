//! [`EmailSupport`] over the `omni_email` library: activity recording, the
//! retry queue, calendar-scoped sender rules, the shared triage and activity log
//! capture.

use futures::future::BoxFuture;
use omni_core::email::FetchedEmail;
use omni_email::activity::{
    self, ActivityEmail, AdmitTier as EmailAdmitTier, EmailActivityOutcome, EmailPipelineName,
    LlmCost, NewActivity,
};
use omni_email::sender_rules::{self, RuleTarget};
use omni_email::triage::{EmailTriage, TriageEmail};
use omni_email::{activity_logs, retry};
use omni_store::Store;
use omni_tasks::RunLogs;

use crate::support::{
    ActivityEntry, ActivityOutcome, AdmitTier, CapturedWork, CostCents, EmailSupport, RuleVerdict,
    SenderRuleMatch, SupportError, TriageVerdict,
};

/// The production email core for the calendar pipeline.
#[derive(Clone)]
pub struct OmniEmailSupport {
    store: Store,
    run_logs: RunLogs,
    triage: EmailTriage,
}

impl OmniEmailSupport {
    /// `triage` must be the instance shared with the parcel pipeline so one
    /// email is classified once.
    pub fn new(store: Store, run_logs: RunLogs, triage: EmailTriage) -> Self {
        Self {
            store,
            run_logs,
            triage,
        }
    }
}

fn store_error(operation: &str) -> impl FnOnce(omni_store::StoreError) -> SupportError + '_ {
    move |error| SupportError::new(format!("{operation} failed: {error}"))
}

fn pipeline_name(pipeline: &str) -> EmailPipelineName {
    EmailPipelineName::parse(pipeline).unwrap_or(EmailPipelineName::CalendarEvents)
}

fn outcome(outcome: ActivityOutcome) -> EmailActivityOutcome {
    match outcome {
        ActivityOutcome::Filtered => EmailActivityOutcome::Filtered,
        ActivityOutcome::NoMatches => EmailActivityOutcome::NoMatches,
        ActivityOutcome::Processed => EmailActivityOutcome::Processed,
        ActivityOutcome::Partial => EmailActivityOutcome::Partial,
        ActivityOutcome::Failed => EmailActivityOutcome::Failed,
        ActivityOutcome::Error => EmailActivityOutcome::Error,
    }
}

fn admit_tier(tier: AdmitTier) -> EmailAdmitTier {
    match tier {
        AdmitTier::Rule => EmailAdmitTier::Rule,
        AdmitTier::Builtin => EmailAdmitTier::Builtin,
        AdmitTier::Triage => EmailAdmitTier::Triage,
        AdmitTier::KeywordFallback => EmailAdmitTier::KeywordFallback,
    }
}

/// `undefined` → omitted, `null` → unpriced, number → cents.
fn llm_cost(cost: CostCents) -> LlmCost {
    match cost {
        None => LlmCost::None,
        Some(None) => LlmCost::Unpriced,
        Some(Some(cents)) => LlmCost::Cents(cents),
    }
}

/// The activity row `recordEmailActivity` writes for an entry.
pub fn new_activity(entry: ActivityEntry) -> NewActivity {
    NewActivity {
        pipeline: pipeline_name(entry.pipeline),
        email: ActivityEmail {
            id: entry.email_id,
            subject: entry.subject,
            from: entry.from,
            received_at: entry.received_at,
        },
        outcome: outcome(entry.outcome),
        detail: entry.detail,
        admit_reason: entry.admit_reason,
        admit_tier: entry.admit_tier.map(admit_tier),
        cost_cents: llm_cost(entry.cost_cents),
        items: entry.items,
    }
}

impl EmailSupport for OmniEmailSupport {
    fn find_sender_rule<'a>(
        &'a self,
        from: &'a str,
    ) -> BoxFuture<'a, Result<Option<SenderRuleMatch>, SupportError>> {
        Box::pin(async move {
            let rule = sender_rules::find_sender_rule(&self.store, from, RuleTarget::Calendar)
                .await
                .map_err(store_error("find sender rule"))?;
            Ok(rule.map(|rule| SenderRuleMatch {
                pattern: rule.pattern,
                verdict: match rule.verdict {
                    sender_rules::RuleVerdict::Allow => RuleVerdict::Allow,
                    sender_rules::RuleVerdict::Block => RuleVerdict::Block,
                },
            }))
        })
    }

    fn classify<'a>(
        &'a self,
        email: &'a FetchedEmail,
    ) -> BoxFuture<'a, Result<TriageVerdict, SupportError>> {
        Box::pin(async move {
            let verdict = self
                .triage
                .classify(&TriageEmail::from(email))
                .await
                .map_err(|error| SupportError::new(error.message))?;
            Ok(TriageVerdict {
                parcel: verdict.parcel,
                calendar: verdict.calendar,
                reason: verdict.reason,
            })
        })
    }

    fn triage_cost_cents(&self, email_id: &str) -> Option<f64> {
        self.triage.triage_cost_cents(email_id).as_nullable()
    }

    fn record_activity(&self, entry: ActivityEntry) -> BoxFuture<'_, Result<(), SupportError>> {
        Box::pin(async move {
            activity::record(&self.store, new_activity(entry))
                .await
                .map(drop)
                .map_err(store_error("record email activity"))
        })
    }

    fn enqueue_retry<'a>(
        &'a self,
        pipeline: &'static str,
        email_id: &'a str,
        reason: &'a str,
    ) -> BoxFuture<'a, Result<(), SupportError>> {
        Box::pin(async move {
            retry::enqueue(&self.store, pipeline, email_id, reason)
                .await
                .map(drop)
                .map_err(store_error("enqueue calendar email retry"))
        })
    }

    fn with_log_capture<'a>(
        &'a self,
        activity_id: String,
        pipeline: &'static str,
        work: CapturedWork<'a>,
    ) -> CapturedWork<'a> {
        Box::pin(async move {
            activity_logs::with_capture(&self.store, &self.run_logs, &activity_id, pipeline, work)
                .await
        })
    }
}
