//! User sender allow/block rules (`src/email/senderRules.ts`). User rules beat
//! the built-in lists in both directions; among matching rules block beats allow.

use std::collections::BTreeSet;

use omni_store::cbor::Extra;
use omni_store::entity::{Entity, EntityOps as _, EntityWrite as _, UpsertOpts};
use omni_store::{Store, StoreError};
use serde::{Deserialize, Serialize};

/// The pipeline a rule lookup is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleTarget {
    Parcel,
    Calendar,
}

impl RuleTarget {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Parcel => "parcel",
            Self::Calendar => "calendar",
        }
    }

    fn opposite(self) -> Self {
        match self {
            Self::Parcel => Self::Calendar,
            Self::Calendar => Self::Parcel,
        }
    }
}

pub use omni_api::email::{RuleScope, RuleVerdict};
pub use omni_core::js::{is_js_whitespace as is_js_space, trim as js_trim};

/// Whether a rule of this scope applies to `target`.
pub fn scope_covers(scope: RuleScope, target: RuleTarget) -> bool {
    match scope {
        RuleScope::Both => true,
        RuleScope::Parcel => target == RuleTarget::Parcel,
        RuleScope::Calendar => target == RuleTarget::Calendar,
    }
}

fn scope_single(scope: RuleScope) -> Option<RuleTarget> {
    match scope {
        RuleScope::Parcel => Some(RuleTarget::Parcel),
        RuleScope::Calendar => Some(RuleTarget::Calendar),
        RuleScope::Both => None,
    }
}

impl From<RuleTarget> for RuleScope {
    fn from(target: RuleTarget) -> Self {
        match target {
            RuleTarget::Parcel => Self::Parcel,
            RuleTarget::Calendar => Self::Calendar,
        }
    }
}

/// `EmailRuleData` (entity `email-sender-rule`, key `ruleId`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailRuleData {
    /// `<scope>:<pattern>`.
    pub rule_id: String,
    /// Lowercase full address, `@host` domain rule, or legacy bare domain.
    pub pattern: String,
    pub scope: RuleScope,
    pub verdict: RuleVerdict,
    pub created_at: i64,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity for EmailRuleData {
    const NAME: &'static str = "email-sender-rule";
    type Key = String;
    fn key(&self) -> String {
        self.rule_id.clone()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RuleError {
    #[error("Sender rule pattern must be non-empty")]
    EmptyPattern,
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// Address portion of a lowercased sender; tolerates `Name <user@host>`.
fn sender_address(from_lower: &str) -> &str {
    bracketed(from_lower).unwrap_or(from_lower).trim()
}

/// The first `<...>` group (`/<([^>]*)>/`).
fn bracketed(s: &str) -> Option<&str> {
    let start = s.find('<')?;
    let rest = &s[start + 1..];
    rest.find('>').map(|end| &rest[..end])
}

fn sender_domain(from_lower: &str) -> &str {
    let addr = sender_address(from_lower);
    match addr.rfind('@') {
        Some(at) => addr[at + 1..].trim(),
        None => addr,
    }
}

fn domain_matches(sender_domain: &str, domain: &str) -> bool {
    sender_domain == domain
        || sender_domain
            .strip_suffix(domain)
            .is_some_and(|prefix| prefix.ends_with('.'))
}

/// `matchesSenderPattern`: `@host` matches the host and its subdomains,
/// `local@host` matches the exact address, a legacy bare domain matches the
/// host and its subdomains.
pub fn matches_sender_pattern(from_lower: &str, pattern: &str) -> bool {
    if let Some(domain) = pattern.strip_prefix('@') {
        return domain_matches(sender_domain(from_lower), domain);
    }
    if pattern.contains('@') {
        return sender_address(from_lower) == pattern;
    }
    domain_matches(sender_domain(from_lower), pattern)
}

/// `normalizeRulePattern`: strips display-name wrappers, turns a bare domain
/// into `@host`, collapses repeated `@`, keeps full addresses.
pub fn normalize_rule_pattern(input: &str) -> String {
    let mut p = js_trim(input).to_lowercase();
    if p.is_empty() {
        return p;
    }
    if let Some(inner) = bracketed(&p).filter(|inner| !inner.is_empty()) {
        p = js_trim(inner).to_owned();
    }
    if p.starts_with('@') {
        return format!("@{}", p.trim_start_matches('@'));
    }
    if p.contains('@') {
        return p;
    }
    format!("@{p}")
}

/// The rule deciding `from` for `target` (scope matches or is `both`); block
/// beats allow, otherwise the first match in storage order.
pub fn select_sender_rule<'a>(
    rules: &'a [EmailRuleData],
    from: &str,
    target: RuleTarget,
) -> Option<&'a EmailRuleData> {
    let from_lower = from.to_lowercase();
    let matches: Vec<&EmailRuleData> = rules
        .iter()
        .filter(|rule| {
            scope_covers(rule.scope, target) && matches_sender_pattern(&from_lower, &rule.pattern)
        })
        .collect();
    matches
        .iter()
        .find(|rule| rule.verdict == RuleVerdict::Block)
        .or_else(|| matches.first())
        .copied()
}

/// `findSenderRule`.
pub async fn find_sender_rule(
    store: &Store,
    from: &str,
    target: RuleTarget,
) -> Result<Option<EmailRuleData>, StoreError> {
    let rules = store.read(|docs| docs.get_all::<EmailRuleData>()).await?;
    Ok(select_sender_rule(&rules, from, target).cloned())
}

/// `getSenderRuleVerdict`.
pub async fn sender_rule_verdict(
    store: &Store,
    from: &str,
    target: RuleTarget,
) -> Result<Option<RuleVerdict>, StoreError> {
    Ok(find_sender_rule(store, from, target)
        .await?
        .map(|r| r.verdict))
}

/// `listEmailRules`: newest first.
pub async fn list(store: &Store) -> Result<Vec<EmailRuleData>, StoreError> {
    let mut rules = store.read(|docs| docs.get_all::<EmailRuleData>()).await?;
    rules.sort_by_key(|a| std::cmp::Reverse(a.created_at));
    Ok(rules)
}

/// `upsertEmailRule` (raw; user-facing creation uses [`upsert_checked`]).
pub async fn upsert(
    store: &Store,
    pattern: &str,
    scope: RuleScope,
    verdict: RuleVerdict,
) -> Result<EmailRuleData, RuleError> {
    let pattern = js_trim(pattern).to_lowercase();
    if pattern.is_empty() {
        return Err(RuleError::EmptyPattern);
    }
    let now = store.clock().now_ms();
    store
        .write(move |tx| {
            let rule_id = format!("{}:{pattern}", scope.as_str());
            let existing = tx.get::<EmailRuleData>(&rule_id)?;
            let row = EmailRuleData {
                rule_id,
                pattern,
                scope,
                verdict,
                created_at: existing.as_ref().map_or(now, |e| e.created_at),
                extra: existing.map(|e| e.extra).unwrap_or_default(),
            };
            tx.upsert(&row, UpsertOpts::default())?;
            Ok::<_, RuleError>(row)
        })
        .await
}

/// `deleteEmailRule`: `true` when the rule existed.
pub async fn delete(store: &Store, rule_id: &str) -> Result<bool, StoreError> {
    let key = rule_id.to_owned();
    store
        .write(move |tx| tx.delete::<EmailRuleData>(&key))
        .await
}

/// Existing user-rule coverage for an exact pattern.
#[derive(Clone, Debug, PartialEq)]
pub struct RuleCoverage {
    pub pattern: String,
    pub blocked_scopes: BTreeSet<RuleTarget>,
    pub allowed_scopes: BTreeSet<RuleTarget>,
    pub has_both_rule: bool,
    pub matches: Vec<EmailRuleData>,
}

/// `getSenderRuleCoverage` over a rule list.
pub fn coverage(rules: &[EmailRuleData], pattern: &str) -> RuleCoverage {
    let normalized = js_trim(pattern).to_lowercase();
    let matches: Vec<EmailRuleData> = rules
        .iter()
        .filter(|rule| rule.pattern == normalized)
        .cloned()
        .collect();
    let mut blocked_scopes = BTreeSet::new();
    let mut allowed_scopes = BTreeSet::new();
    let mut has_both_rule = false;
    for rule in &matches {
        let scopes = match scope_single(rule.scope) {
            Some(target) => vec![target],
            None => {
                has_both_rule = true;
                vec![RuleTarget::Parcel, RuleTarget::Calendar]
            }
        };
        let set = match rule.verdict {
            RuleVerdict::Block => &mut blocked_scopes,
            RuleVerdict::Allow => &mut allowed_scopes,
        };
        set.extend(scopes);
    }
    RuleCoverage {
        pattern: normalized,
        blocked_scopes,
        allowed_scopes,
        has_both_rule,
        matches,
    }
}

/// `RuleAddPlan`.
#[derive(Clone, Debug, PartialEq)]
pub enum RuleAddPlan {
    Create(EmailRuleData),
    UpgradeToBoth {
        delete: Vec<EmailRuleData>,
        row: EmailRuleData,
    },
    NoopExists(EmailRuleData),
}

fn new_rule(
    scope: RuleScope,
    pattern: &str,
    verdict: RuleVerdict,
    created_at: i64,
) -> EmailRuleData {
    EmailRuleData {
        rule_id: format!("{}:{pattern}", scope.as_str()),
        pattern: pattern.to_owned(),
        scope,
        verdict,
        created_at,
        extra: Extra::new(),
    }
}

/// `planRuleAdd`: pure decision over the current rules.
pub fn plan_rule_add(
    rules: &[EmailRuleData],
    pattern: &str,
    scope: RuleScope,
    verdict: RuleVerdict,
    now: i64,
) -> Result<RuleAddPlan, RuleError> {
    let normalized = js_trim(pattern).to_lowercase();
    if normalized.is_empty() {
        return Err(RuleError::EmptyPattern);
    }
    let cov = coverage(rules, &normalized);

    // A "both" rule is authoritative: fold existing single-scope rows into it.
    let Some(target) = scope_single(scope) else {
        let singles: Vec<EmailRuleData> = cov
            .matches
            .iter()
            .filter(|r| r.scope != RuleScope::Both)
            .cloned()
            .collect();
        let existing_both = cov.matches.iter().find(|r| r.scope == RuleScope::Both);
        if let Some(existing) = existing_both
            && existing.verdict == verdict
            && singles.is_empty()
        {
            return Ok(RuleAddPlan::NoopExists(existing.clone()));
        }
        let created_at = match existing_both {
            Some(existing) if existing.verdict == verdict => existing.created_at,
            _ => cov.matches.iter().map(|r| r.created_at).fold(now, i64::min),
        };
        let row = new_rule(RuleScope::Both, &normalized, verdict, created_at);
        return Ok(if singles.is_empty() {
            RuleAddPlan::Create(row)
        } else {
            RuleAddPlan::UpgradeToBoth {
                delete: singles,
                row,
            }
        });
    };

    if let Some(existing) = cov
        .matches
        .iter()
        .find(|r| r.scope == RuleScope::Both && r.verdict == verdict)
    {
        return Ok(RuleAddPlan::NoopExists(existing.clone()));
    }
    if let Some(existing) = cov
        .matches
        .iter()
        .find(|r| r.scope == scope && r.verdict == verdict)
    {
        return Ok(RuleAddPlan::NoopExists(existing.clone()));
    }
    let opposite_scope = RuleScope::from(target.opposite());
    if let Some(opposite) = cov
        .matches
        .iter()
        .find(|r| r.scope == opposite_scope && r.verdict == verdict)
    {
        return Ok(RuleAddPlan::UpgradeToBoth {
            delete: vec![opposite.clone()],
            row: new_rule(RuleScope::Both, &normalized, verdict, opposite.created_at),
        });
    }
    Ok(RuleAddPlan::Create(new_rule(
        scope,
        &normalized,
        verdict,
        now,
    )))
}

/// `UpsertEmailRuleCheckedResult`.
#[derive(Clone, Debug, PartialEq)]
pub struct CheckedUpsert {
    pub rule: EmailRuleData,
    /// Two single-scope rules were merged into one `both` row.
    pub merged: bool,
    /// An identical (or broader) rule already existed; nothing changed.
    pub already_exists: bool,
}

/// `upsertEmailRuleChecked`: plans and applies in one transaction. The
/// superseding `both` row is written before the single-scope rows are deleted.
pub async fn upsert_checked(
    store: &Store,
    pattern: &str,
    scope: RuleScope,
    verdict: RuleVerdict,
) -> Result<CheckedUpsert, RuleError> {
    let now = store.clock().now_ms();
    let pattern = pattern.to_owned();
    store
        .write(move |tx| {
            let rules = tx.get_all::<EmailRuleData>()?;
            match plan_rule_add(&rules, &pattern, scope, verdict, now)? {
                RuleAddPlan::NoopExists(existing) => Ok(CheckedUpsert {
                    rule: existing,
                    merged: false,
                    already_exists: true,
                }),
                RuleAddPlan::UpgradeToBoth { delete, row } => {
                    tx.upsert(&row, UpsertOpts::default())?;
                    for stale in &delete {
                        tx.delete::<EmailRuleData>(&stale.rule_id)?;
                    }
                    Ok(CheckedUpsert {
                        rule: row,
                        merged: true,
                        already_exists: false,
                    })
                }
                RuleAddPlan::Create(row) => {
                    tx.upsert(&row, UpsertOpts::default())?;
                    Ok(CheckedUpsert {
                        rule: row,
                        merged: false,
                        already_exists: false,
                    })
                }
            }
        })
        .await
}

/// Representative senders a user rule for `pattern` targets (a domain rule
/// also targets subdomains).
fn rule_sample_senders(pattern: &str) -> Vec<String> {
    let domain = match pattern.strip_prefix('@') {
        Some(domain) => Some(domain),
        None if pattern.contains('@') => None,
        None => Some(pattern),
    };
    match domain {
        None => vec![pattern.to_owned()],
        Some(domain) => vec![format!("probe@{domain}"), format!("probe@sub.{domain}")],
    }
}

/// `matchesBuiltinBlock`: a block rule is redundant when a built-in
/// blacklist already covers every sender it targets.
pub fn matches_builtin_block(pattern: &str, scope: RuleScope) -> bool {
    let samples = rule_sample_senders(pattern);
    let covered_by = |list: &[&str]| {
        samples
            .iter()
            .all(|s| list.iter().any(|entry| s.contains(&entry.to_lowercase())))
    };
    let parcel = covered_by(crate::builtin::PARCEL_BLACKLISTED_SENDERS);
    let calendar = covered_by(crate::builtin::CALENDAR_BLACKLISTED_SENDERS);
    match scope {
        RuleScope::Parcel => parcel,
        RuleScope::Calendar => calendar,
        RuleScope::Both => parcel && calendar,
    }
}

#[cfg(test)]
mod pure_tests {
    use super::*;

    #[test]
    fn matches_a_full_address_exactly() {
        assert!(matches_sender_pattern("orders@shop.com", "orders@shop.com"));
    }

    #[test]
    fn does_not_match_a_different_mailbox_for_a_full_address_pattern() {
        assert!(!matches_sender_pattern(
            "noreply@shop.com",
            "orders@shop.com"
        ));
    }

    #[test]
    fn matches_every_mailbox_for_an_at_domain_style_pattern() {
        assert!(matches_sender_pattern("orders@shop.com", "@shop.com"));
        assert!(matches_sender_pattern("noreply@shop.com", "@shop.com"));
    }

    #[test]
    fn matches_subdomains_for_an_at_domain_style_pattern() {
        assert!(matches_sender_pattern("noreply@mail.shop.com", "@shop.com"));
        assert!(matches_sender_pattern("a@deep.mail.shop.com", "@shop.com"));
        assert!(!matches_sender_pattern("a@notshop.com", "@shop.com"));
    }

    #[test]
    fn matches_a_bare_domain_against_the_senders_domain() {
        assert!(matches_sender_pattern("orders@shop.com", "shop.com"));
    }

    #[test]
    fn matches_subdomains_for_a_bare_domain_pattern() {
        assert!(matches_sender_pattern("noreply@mail.shop.com", "shop.com"));
    }

    #[test]
    fn does_not_match_a_lookalike_domain_for_a_bare_domain_pattern() {
        assert!(!matches_sender_pattern("orders@notshop.com", "shop.com"));
    }

    #[test]
    fn tolerates_the_display_name_angle_bracket_form() {
        assert!(matches_sender_pattern(
            "\"shop\" <orders@shop.com>",
            "shop.com"
        ));
        assert!(matches_sender_pattern(
            "\"shop\" <orders@shop.com>",
            "orders@shop.com"
        ));
    }

    #[test]
    fn prefixes_a_bare_domain_with_at() {
        assert_eq!(normalize_rule_pattern("plex.tv"), "@plex.tv");
    }

    #[test]
    fn keeps_an_at_domain_pattern_collapsing_casing_and_extra_at() {
        assert_eq!(normalize_rule_pattern("@Plex.TV"), "@plex.tv");
        assert_eq!(normalize_rule_pattern("@@plex.tv"), "@plex.tv");
    }

    #[test]
    fn keeps_a_full_address_as_an_exact_address_rule() {
        assert_eq!(
            normalize_rule_pattern("Orders@DoorDash.com"),
            "orders@doordash.com"
        );
    }

    #[test]
    fn strips_a_display_name_wrapper() {
        assert_eq!(
            normalize_rule_pattern("Plex <no-reply@plex.tv>"),
            "no-reply@plex.tv"
        );
    }

    #[test]
    fn builtin_block_coverage_requires_every_sample() {
        // A domain rule also targets subdomains, which "@npmjs.com" does not cover.
        assert!(!matches_builtin_block("@npmjs.com", RuleScope::Both));
        assert!(matches_builtin_block("support@npmjs.com", RuleScope::Both));
        assert!(matches_builtin_block("orders@amazon.ca", RuleScope::Parcel));
        assert!(!matches_builtin_block("orders@amazon.ca", RuleScope::Both));
        assert!(!matches_builtin_block("@shop.com", RuleScope::Parcel));
        assert!(matches_builtin_block("noreply@github.com", RuleScope::Both));
    }
}
