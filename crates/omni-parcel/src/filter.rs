//! Parcel candidate filter (`src/parcel-tracker/filter/keywords.ts`). Order:
//! user block, user allow, static blacklist (and the user's own address),
//! AliExpress order-status block, static carrier auto-pass, then shared LLM
//! triage; tracking keywords and carrier names are only the fallback when
//! triage is unavailable.

use omni_email::activity::AdmitTier;
use omni_email::builtin::{PARCEL_BLACKLISTED_SENDERS, PARCEL_CARRIER_SENDER_DOMAINS};
use omni_email::sender_rules::{RuleTarget, RuleVerdict, find_sender_rule};
use omni_email::triage::{EmailTriage, TriageEmail};
use omni_store::{Store, StoreError};

use crate::carriers::carrier_map::CarrierDirectory;

const TRACKING_KEYWORDS: &[&str] = &[
    "tracking",
    "track",
    "shipped",
    "out for delivery",
    "tracking number",
    "order shipped",
    "in transit",
    "shipment",
    "estimated delivery",
    "delivery confirmation",
    "package",
    "delivered",
    "delivery",
];

/// AliExpress order-status subjects carry no tracking info (the biggest
/// source of wasted extraction calls); "Package ... ready to ship" still passes.
const ALIEXPRESS_ORDER_STATUS_PHRASES: &[&str] = &[
    "awaiting confirmation",
    "order shipped",
    "order confirmed",
    "delivery update",
    "awaiting payment",
];

/// `isAliexpressOrderStatus`.
pub fn is_aliexpress_order_status(from_lower: &str, subject: &str) -> bool {
    if !from_lower.contains("aliexpress") {
        return false;
    }
    let subject_lower = subject.to_lowercase();
    if starts_with_order_number(&subject_lower) {
        return true;
    }
    ALIEXPRESS_ORDER_STATUS_PHRASES
        .iter()
        .any(|phrase| subject_lower.contains(phrase))
}

/// `/^order \d+:/`.
fn starts_with_order_number(subject_lower: &str) -> bool {
    let Some(rest) = subject_lower.strip_prefix("order ") else {
        return false;
    };
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    digits > 0 && rest[digits..].starts_with(':')
}

/// The filter verdict.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FilterResult {
    Pass {
        reason: String,
        admit_tier: AdmitTier,
    },
    Skip {
        reason: String,
    },
}

impl FilterResult {
    fn pass(reason: impl Into<String>, admit_tier: AdmitTier) -> Self {
        FilterResult::Pass {
            reason: reason.into(),
            admit_tier,
        }
    }

    fn skip(reason: impl Into<String>) -> Self {
        FilterResult::Skip {
            reason: reason.into(),
        }
    }

    pub fn reason(&self) -> &str {
        match self {
            FilterResult::Pass { reason, .. } | FilterResult::Skip { reason } => reason,
        }
    }
}

/// What the filter reads.
pub struct FilterDeps<'a> {
    pub store: &'a Store,
    pub triage: &'a EmailTriage,
    pub carriers: &'a CarrierDirectory,
    /// `EMAIL_SELF_ADDRESS` (the user's own outgoing mail is never a shipment).
    pub self_address: Option<&'a str>,
}

fn is_blacklisted_sender(from_lower: &str, self_address: Option<&str>) -> bool {
    if PARCEL_BLACKLISTED_SENDERS
        .iter()
        .any(|sender| from_lower.contains(sender))
    {
        return true;
    }
    self_address.is_some_and(|own| from_lower.contains(&own.to_lowercase()))
}

/// `filterTrackingCandidateEffect`.
pub async fn filter_tracking_candidate(
    deps: &FilterDeps<'_>,
    email: &TriageEmail,
) -> Result<FilterResult, StoreError> {
    let from_lower = email.from.to_lowercase();

    // User rules beat the built-in lists in both directions.
    if let Some(rule) = find_sender_rule(deps.store, &email.from, RuleTarget::Parcel).await? {
        return Ok(match rule.verdict {
            RuleVerdict::Block => FilterResult::skip(format!("blocked by rule {}", rule.pattern)),
            RuleVerdict::Allow => {
                FilterResult::pass(format!("allowed by rule {}", rule.pattern), AdmitTier::Rule)
            }
        });
    }
    if is_blacklisted_sender(&from_lower, deps.self_address) {
        return Ok(FilterResult::skip("blacklisted sender"));
    }
    if is_aliexpress_order_status(&from_lower, &email.subject) {
        return Ok(FilterResult::skip("aliexpress order-status"));
    }
    if PARCEL_CARRIER_SENDER_DOMAINS
        .iter()
        .any(|domain| from_lower.contains(domain))
    {
        return Ok(FilterResult::pass("carrier sender", AdmitTier::Builtin));
    }

    // Cheap-LLM triage decides everything else; keywords are only the fallback.
    match deps.triage.classify(email).await {
        Ok(verdict) if verdict.parcel => Ok(FilterResult::pass(
            format!("triage: {}", verdict.reason),
            AdmitTier::Triage,
        )),
        Ok(verdict) => Ok(FilterResult::skip(format!("triage: {}", verdict.reason))),
        Err(_) => Ok(keyword_fallback(deps.carriers, email).await),
    }
}

/// Degraded path when the triage model is unavailable.
async fn keyword_fallback(carriers: &CarrierDirectory, email: &TriageEmail) -> FilterResult {
    let search_text = format!("{} {}", email.subject, email.text_body).to_lowercase();
    if let Some(keyword) = TRACKING_KEYWORDS
        .iter()
        .find(|kw| search_text.contains(*kw))
    {
        return FilterResult::pass(
            format!("keyword \"{keyword}\" (triage unavailable)"),
            AdmitTier::KeywordFallback,
        );
    }
    let full_text = format!("{} {}", email.subject, email.text_body);
    if carriers
        .name_patterns()
        .await
        .iter()
        .any(|pattern| pattern.is_match(&full_text))
    {
        return FilterResult::pass(
            "carrier name match (triage unavailable)",
            AdmitTier::CarrierName,
        );
    }
    FilterResult::skip("no keyword match (triage unavailable)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_order_number_subjects() {
        assert!(starts_with_order_number("order 8196234512: view details"));
        assert!(!starts_with_order_number("order : x"));
        assert!(!starts_with_order_number("your order 1: x"));
    }
}
