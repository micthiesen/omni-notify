//! Stylist (`/stylist`): a read-only view of the fit-first stylist ledger
//! (`GET /api/stylist`). What needs the owner, body measurements, the
//! wardrobe with its fit evidence, fit history, frozen forecasts and how they
//! scored, rules, preferences, wardrobe needs, backups and the runbook.
//!
//! `?ns=` picks the namespace: `real` is the owner's ledger, `fixture-*`
//! namespaces hold synthetic test data. The overview carries no photos or
//! evidence bytes and this page never asks for any.

use std::time::Duration;

use leptos::prelude::*;
use omni_api::stylist::{
    StylistArchive, StylistBodyMeasurement, StylistCase, StylistHistory, StylistMeasurement,
    StylistNamespace, StylistNeed, StylistOutcome, StylistOverview, StylistPiece,
    StylistPrediction, StylistPreference, StylistProfile, StylistRegion, StylistRule,
    StylistRunbook, StylistScore, StylistWarning,
};
use omni_web_kit::api::{self, ApiClientError};
use omni_web_kit::components::{
    Disclosure, EmptyState, ErrorState, Glyph, Icon, IconSize, PageHead, Panel, Readout,
    ReadoutBand, SegOption, Segmented, ShowMoreButton, SkeletonRows, Status, StatusKind, Tag, Tone,
    use_show_more,
};
use omni_web_kit::hooks::{query_param, replace_query_param, use_visible_poll};
use omni_web_kit::utils::days::{parse_ymd, short_date};
use omni_web_kit::utils::format::{format_absolute, format_relative_at};
use omni_web_kit::utils::js::{local_day_index, now_ms, number_string, parse_date_ms, to_fixed};

/// The owner's ledger.
pub const REAL: &str = "real";
/// How often an open page re-reads the ledger (assistants write it over MCP).
const POLL: Duration = Duration::from_secs(60);
/// Fit cases shown before "Show more".
const CASES_SHOWN: usize = 8;
/// Archives listed before the rest collapse.
const ARCHIVES_SHOWN: usize = 4;
/// A backup that trails the ledger for longer than this is worth a warning
/// (`StylistBackup` runs hourly).
const BACKUP_GRACE_MS: f64 = 2.0 * 3_600_000.0;

// ----- words -----

/// `real` or `fixture-<slug>`: the server's namespace pattern.
pub fn valid_namespace(ns: &str) -> bool {
    if ns == REAL {
        return true;
    }
    let Some(slug) = ns.strip_prefix("fixture-") else {
        return false;
    };
    let lower_or_digit = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    slug.len() <= 40
        && slug.bytes().next().is_some_and(lower_or_digit)
        && slug.bytes().all(|b| lower_or_digit(b) || b == b'-')
}

/// `1 piece`, `3 pieces`.
pub fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

/// `body_length` → `body length`.
pub fn words(slug: &str) -> String {
    slug.replace('_', " ")
}

/// First letter upper-cased.
pub fn sentence_case(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// `tops/tshirt` → `Tops / tshirt`; `any` → `Any category`.
pub fn category_label(category: &str) -> String {
    if category == "any" {
        return "Any category".to_owned();
    }
    sentence_case(
        &category
            .split('/')
            .map(words)
            .collect::<Vec<_>>()
            .join(" / "),
    )
}

/// `a, b and c`.
fn join_and(parts: &[String]) -> String {
    match parts {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// A `YYYY-MM-DD` (or ISO timestamp) day as `Oct 3`, with the year outside
/// `today`'s; other text as is.
pub fn date_label(date: &str, today: i64) -> String {
    parse_ymd(date).map_or_else(|| date.to_owned(), |day| short_date(day, today))
}

/// Millimetres as centimetres to one decimal (`965` → `96.5`).
pub fn cm(value_mm: f64) -> String {
    to_fixed(value_mm / 10.0, 1)
}

/// Whether the value as recorded says more than the centimetres shown:
/// another unit, or more precision.
pub fn recorded_differs(original: &str, value_mm: f64) -> bool {
    let Some((value, unit)) = original.trim().split_once(' ') else {
        return true;
    };
    let decimals = value.split_once('.').map_or(0, |(_, d)| d.len());
    !(unit == "cm"
        && decimals <= 1
        && value
            .parse::<f64>()
            .is_ok_and(|v| (v * 10.0 - value_mm).abs() < 0.01))
}

/// `1.4 KB`.
pub fn format_bytes(bytes: f64) -> String {
    if bytes < 1024.0 {
        format!("{} B", number_string(bytes))
    } else if bytes < 1024.0 * 1024.0 {
        format!("{} KB", to_fixed(bytes / 1024.0, 1))
    } else {
        format!("{} MB", to_fixed(bytes / (1024.0 * 1024.0), 1))
    }
}

/// The CSS text class for a tone (none when neutral).
fn tone_class(tone: Tone) -> &'static str {
    match tone {
        Tone::Warn => "text-warn",
        Tone::Fault => "text-fault",
        _ => "",
    }
}

/// The CSS text class for a tone (`muted` when neutral).
fn tone_text(tone: Tone) -> &'static str {
    match tone {
        Tone::Warn => "text-warn",
        Tone::Fault => "text-fault",
        _ => "muted",
    }
}

// ----- looks -----

/// A piece's fit evidence: shape and word.
pub fn fit_look(fit: &str) -> (StatusKind, &'static str) {
    match fit {
        "proven" => (StatusKind::Ok, "Proven fit"),
        "failed" => (StatusKind::Fault, "Failed fit"),
        "mixed" => (StatusKind::Warn, "Mixed fit"),
        _ => (StatusKind::Idle, "Untested"),
    }
}

/// A region verdict (`good`, `slightly_tight`, `too_long`, ...): good is a
/// quiet dot, a slight miss a diamond, a clear miss a square.
pub fn verdict_kind(verdict: &str) -> StatusKind {
    match verdict {
        "good" => StatusKind::Ok,
        v if v.starts_with("slightly_") => StatusKind::Warn,
        v if v.starts_with("too_") => StatusKind::Fault,
        _ => StatusKind::Idle,
    }
}

/// A forecast region scored against an outcome.
pub fn match_kind(matched: &str) -> StatusKind {
    match matched {
        "exact" => StatusKind::Ok,
        "close" => StatusKind::Warn,
        "miss" => StatusKind::Fault,
        _ => StatusKind::Idle,
    }
}

/// An outcome stage's result.
pub fn result_look(result: &str) -> (StatusKind, &'static str) {
    match result {
        "matched" => (StatusKind::Ok, "Matched"),
        "partial" => (StatusKind::Warn, "Partial"),
        "missed" => (StatusKind::Fault, "Missed"),
        "not_purchased" => (StatusKind::Idle, "Not purchased"),
        _ => (StatusKind::Idle, "Unresolved"),
    }
}

/// A forecast's decision.
pub fn decision_look(decision: &str) -> (StatusKind, &'static str) {
    match decision {
        "recommend" => (StatusKind::Ok, "Recommend"),
        "recommend_with_caution" => (StatusKind::Warn, "With caution"),
        _ => (StatusKind::Idle, "Abstain"),
    }
}

fn basis_rank(basis: &str) -> u8 {
    match basis {
        "exact_repeat" => 2,
        "known_brand_new_cut" => 1,
        _ => 0,
    }
}

/// `Exact repeat`, `Known brand, new cut`, `New brand inference`.
pub fn basis_label(basis: &str) -> String {
    match basis {
        "exact_repeat" => "Exact repeat".to_owned(),
        "known_brand_new_cut" => "Known brand, new cut".to_owned(),
        "new_brand_inference" => "New brand inference".to_owned(),
        other => sentence_case(&words(other)),
    }
}

/// The basis line of a forecast; warn when the claim is stronger than what
/// the ledger computed.
pub fn basis_phrase(claimed: &str, computed: Option<&str>) -> (Tone, String) {
    match computed.filter(|c| *c != claimed) {
        Some(computed) => (
            if basis_rank(claimed) > basis_rank(computed) {
                Tone::Warn
            } else {
                Tone::Neutral
            },
            format!(
                "Claimed {} · computed {}",
                basis_label(claimed).to_lowercase(),
                basis_label(computed).to_lowercase()
            ),
        ),
        None => (Tone::Neutral, basis_label(claimed)),
    }
}

/// `Low return risk`; high is a warning.
pub fn risk_phrase(risk: &str) -> (Tone, String) {
    match risk {
        "unknown" | "" => (Tone::Neutral, "Return risk unknown".to_owned()),
        "high" => (Tone::Warn, "High return risk".to_owned()),
        other => (
            Tone::Neutral,
            format!("{} return risk", sentence_case(other)),
        ),
    }
}

/// `immediate` → `Immediate`, `after_wash` → `After wash`.
pub fn stage_label(stage: &str) -> String {
    sentence_case(&words(stage))
}

/// A fit-history case kind in words.
pub fn case_kind_label(kind: &str) -> String {
    match kind {
        "try_on" => "try-on".to_owned(),
        "kept_unassessed" => "kept, unassessed".to_owned(),
        "forecast_without_outcome" => "forecast, no outcome".to_owned(),
        other => words(other),
    }
}

/// A sizing reference marker (`Baseline ref`).
pub fn reference_label(role: &str) -> String {
    match role {
        "baseline" => "Baseline ref".to_owned(),
        "too_small" => "Too-small ref".to_owned(),
        "too_large" => "Too-large ref".to_owned(),
        "partial" => "Partial ref".to_owned(),
        other => format!("{} ref", sentence_case(&words(other))),
    }
}

/// A piece status worth a tag; owned pieces need none.
pub fn piece_status_tag(status: &str) -> Option<(Tone, &'static str)> {
    match status {
        "undecided" => Some((Tone::Warn, "Undecided")),
        "return_pending" => Some((Tone::Neutral, "Return pending")),
        "returned" => Some((Tone::Neutral, "Returned")),
        "retired" => Some((Tone::Neutral, "Retired")),
        _ => None,
    }
}

/// Owned, undecided or on its way back: still in the wardrobe.
pub fn in_wardrobe(piece: &StylistPiece) -> bool {
    matches!(
        piece.status.as_str(),
        "owned" | "undecided" | "return_pending"
    )
}

/// `Return by Oct 20` with its tone: warn in the last three days, fault once
/// the window has closed.
pub fn return_by_label(return_by: &str, today: i64) -> (Tone, String) {
    let Some(day) = parse_ymd(return_by) else {
        return (Tone::Neutral, format!("Return by {return_by}"));
    };
    let left = day - today;
    let tone = return_tone(left);
    let text = match left {
        l if l < 0 => format!("Return window closed {}", short_date(day, today)),
        0 => "Return by today".to_owned(),
        _ => format!("Return by {}", short_date(day, today)),
    };
    (tone, text)
}

/// The tone of a return deadline `left` days away: warn in the last three
/// days, fault once it has passed. The needs list and the wardrobe share it.
pub fn return_tone(left: i64) -> Tone {
    if left < 0 {
        Tone::Fault
    } else if left <= 3 {
        Tone::Warn
    } else {
        Tone::Neutral
    }
}

/// Days from `today` to a due date, from the date itself when it parses so
/// every panel judges it against the same day; the server's count otherwise.
pub fn days_until(due_on: Option<&str>, server_days_left: Option<f64>, today: i64) -> Option<i64> {
    due_on
        .and_then(parse_ymd)
        .map(|day| day - today)
        .or_else(|| server_days_left.map(|d| d as i64))
}

/// `3 days left`, `today`, `2 days overdue`.
pub fn days_phrase(days: i64) -> String {
    match days {
        0 => "today".to_owned(),
        1 => "1 day left".to_owned(),
        d if d > 1 => format!("{d} days left"),
        -1 => "1 day overdue".to_owned(),
        d => format!("{} days overdue", -d),
    }
}

/// Pieces grouped by category, in the order they arrive.
pub fn group_by_category(pieces: &[StylistPiece]) -> Vec<(String, Vec<StylistPiece>)> {
    let mut groups: Vec<(String, Vec<StylistPiece>)> = Vec::new();
    for piece in pieces {
        match groups.iter_mut().find(|(c, _)| *c == piece.category) {
            Some((_, list)) => list.push(piece.clone()),
            None => groups.push((piece.category.clone(), vec![piece.clone()])),
        }
    }
    groups
}

// ----- attention -----

/// What an attention item is about (the headline names a single kind).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttentionKind {
    Blocking,
    Return,
    Outcome,
    PostWash,
    NeedReview,
    Contradiction,
    Warning,
    Other,
}

impl AttentionKind {
    const ORDER: [AttentionKind; 8] = [
        AttentionKind::Blocking,
        AttentionKind::Return,
        AttentionKind::Outcome,
        AttentionKind::PostWash,
        AttentionKind::NeedReview,
        AttentionKind::Contradiction,
        AttentionKind::Warning,
        AttentionKind::Other,
    ];

    fn noun(self, n: usize) -> String {
        match self {
            AttentionKind::Blocking => plural(n, "blocking warning", "blocking warnings"),
            AttentionKind::Return => plural(n, "return decision", "return decisions"),
            AttentionKind::Outcome => plural(n, "outcome follow-up", "outcome follow-ups"),
            AttentionKind::PostWash => plural(n, "post-wash check", "post-wash checks"),
            AttentionKind::NeedReview => plural(n, "need due for review", "needs due for review"),
            AttentionKind::Contradiction => plural(n, "contradiction", "contradictions"),
            AttentionKind::Warning => plural(n, "ledger check", "ledger checks"),
            AttentionKind::Other => plural(n, "other item", "other items"),
        }
    }
}

/// One row of the "Needs you" panel.
#[derive(Clone, Debug, PartialEq)]
pub struct Attention {
    pub kind: AttentionKind,
    /// The warning code or due kind.
    pub code: String,
    pub status: StatusKind,
    pub word: String,
    pub title: String,
    pub detail: Option<String>,
    /// The date or countdown at the row's end.
    pub when: Option<(Tone, String)>,
}

fn due_item(due: &omni_api::stylist::StylistDue, today: i64) -> Attention {
    let (kind, word) = match due.kind.as_str() {
        "return_decision" => (AttentionKind::Return, "Return decision".to_owned()),
        "outcome_follow_up" => (AttentionKind::Outcome, "Outcome".to_owned()),
        "post_wash_check" => (AttentionKind::PostWash, "After wash".to_owned()),
        "need_review" => (AttentionKind::NeedReview, "Need review".to_owned()),
        other => (AttentionKind::Other, sentence_case(&words(other))),
    };
    let days = days_until(due.due_on.as_deref(), due.days_left, today);
    let (status, when) = if kind == AttentionKind::Return {
        let tone = days.map_or(Tone::Warn, return_tone);
        let status = match tone {
            Tone::Fault => StatusKind::Fault,
            Tone::Neutral => StatusKind::Idle,
            _ => StatusKind::Warn,
        };
        let when = match (due.due_on.as_deref(), days) {
            (Some(on), Some(d)) => Some((
                tone,
                format!("{} · {}", date_label(on, today), days_phrase(d)),
            )),
            (Some(on), None) => Some((tone, date_label(on, today))),
            (None, _) => Some((Tone::Warn, "no deadline recorded".to_owned())),
        };
        (status, when)
    } else {
        let when = due.due_on.as_deref().map(|on| {
            let text = match days {
                Some(0) => "today".to_owned(),
                _ => format!("since {}", date_label(on, today)),
            };
            (Tone::Neutral, text)
        });
        (StatusKind::Idle, when)
    };
    Attention {
        kind,
        code: due.kind.clone(),
        status,
        word,
        title: due.label.clone(),
        detail: (!due.detail.trim().is_empty()).then(|| sentence_case(due.detail.trim())),
        when,
    }
}

/// What needs the owner: blocking warnings, due items, contradictions and
/// ledger warnings. `open_contradictions` is left out while the
/// contradictions themselves are listed.
pub fn attention(o: &StylistOverview, today: i64) -> Vec<Attention> {
    let mut out: Vec<Attention> = o
        .warnings
        .iter()
        .filter(|w| w.severity == "blocking")
        .map(|w| Attention {
            kind: AttentionKind::Blocking,
            code: w.code.clone(),
            status: StatusKind::Fault,
            word: "Blocking".to_owned(),
            title: sentence_case(&w.message),
            detail: Some("Assistants abstain from size advice until this is resolved.".to_owned()),
            when: None,
        })
        .collect();
    out.extend(o.due.iter().map(|d| due_item(d, today)));
    out.extend(o.contradictions.iter().map(|c| Attention {
        kind: AttentionKind::Contradiction,
        code: c.kind.clone(),
        status: StatusKind::Warn,
        word: "Contradiction".to_owned(),
        title: sentence_case(&c.description),
        detail: Some(format!(
            "{} · {}",
            if c.source == "flagged" {
                "Flagged"
            } else {
                "Computed"
            },
            words(&c.kind)
        )),
        when: None,
    }));
    out.extend(
        o.warnings
            .iter()
            .filter(|w| w.severity == "warning")
            .filter(|w| !(w.code == "open_contradictions" && !o.contradictions.is_empty()))
            .map(|w| Attention {
                kind: AttentionKind::Warning,
                code: w.code.clone(),
                status: StatusKind::Warn,
                word: "Check".to_owned(),
                title: sentence_case(&w.message),
                detail: None,
                when: None,
            }),
    );
    out
}

/// Informational ledger notes (freshness), shown with the profile.
pub fn notes(o: &StylistOverview) -> Vec<StylistWarning> {
    o.warnings
        .iter()
        .filter(|w| w.severity != "blocking" && w.severity != "warning")
        .cloned()
        .collect()
}

fn head_for(items: &[Attention]) -> String {
    let n = items.len();
    let Some(first) = items.first() else {
        return String::new();
    };
    if !items.iter().all(|i| i.kind == first.kind) {
        return format!("{n} things need you.");
    }
    match (first.kind, n) {
        (AttentionKind::Blocking, _) => "Size advice is on hold.".to_owned(),
        (AttentionKind::Return, 1) => "1 return decision is open.".to_owned(),
        (AttentionKind::Return, n) => format!("{n} return decisions are open."),
        (AttentionKind::Outcome, 1) => "1 forecast needs its outcome.".to_owned(),
        (AttentionKind::Outcome, n) => format!("{n} forecasts need outcomes."),
        (AttentionKind::PostWash, 1) => "1 post-wash check is due.".to_owned(),
        (AttentionKind::PostWash, n) => format!("{n} post-wash checks are due."),
        (AttentionKind::NeedReview, 1) => "1 need is up for review.".to_owned(),
        (AttentionKind::NeedReview, n) => format!("{n} needs are up for review."),
        (AttentionKind::Contradiction, 1) => "1 contradiction is open.".to_owned(),
        (AttentionKind::Contradiction, n) => format!("{n} contradictions are open."),
        (AttentionKind::Warning, 1) if first.code == "no_profile" => "No profile yet.".to_owned(),
        (AttentionKind::Warning, 1) => "1 ledger check needs a look.".to_owned(),
        (AttentionKind::Warning, n) => format!("{n} ledger checks need a look."),
        (AttentionKind::Other, 1) => "1 thing needs you.".to_owned(),
        (AttentionKind::Other, n) => format!("{n} things need you."),
    }
}

/// Nothing recorded that this page shows: the onboarding state.
pub fn is_blank(o: &StylistOverview) -> bool {
    let p = &o.profile;
    let h = &o.history;
    p.body.is_empty()
        && p.attributes.is_empty()
        && p.sensitivities.is_empty()
        && o.wardrobe.is_empty()
        && o.predictions.is_empty()
        && o.rules.is_empty()
        && o.preferences.is_empty()
        && o.needs.is_empty()
        && o.due.is_empty()
        && o.contradictions.is_empty()
        && h.success_total + h.failure_total + h.unresolved_total == 0.0
}

/// The readout band's figures.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Summary {
    pub pieces: usize,
    pub references: usize,
    pub untested: usize,
    pub body: usize,
    pub stale_body: usize,
    pub newest_body: Option<String>,
    pub successes: usize,
    pub failures: usize,
    pub unresolved: usize,
    /// Forecasts that were not superseded.
    pub forecasts: usize,
    /// Outcomes recorded on current forecasts, scored by region or not.
    pub outcomes: usize,
    /// Region scores that came out exact, of those scored exact, close or miss.
    pub exact: usize,
    pub scored: usize,
}

/// What the recorded outcomes say: nothing yet, outcomes without any region
/// score (not purchased, unresolved or unknown only), or the exact share.
pub fn forecast_scores_phrase(outcomes: usize, exact: usize, scored: usize) -> String {
    match (outcomes, scored) {
        (0, _) => "no outcomes yet".to_owned(),
        (1, 0) => "1 outcome, no region scored".to_owned(),
        (n, 0) => format!("{n} outcomes, no region scored"),
        (_, n) => format!("{exact} of {n} regions exact"),
    }
}

/// Outcomes, exact and scored region counts over `predictions`.
fn forecast_scores<'a>(
    predictions: impl Iterator<Item = &'a StylistPrediction>,
) -> (usize, usize, usize) {
    predictions.flat_map(|p| p.outcomes.iter()).fold(
        (0, 0, 0),
        |(outcomes, exact, scored), outcome| {
            let regions = outcome
                .scores
                .iter()
                .filter(|s| matches!(s.matched.as_str(), "exact" | "close" | "miss"));
            let (e, n) = regions.fold((0, 0), |(e, n), s| {
                (e + usize::from(s.matched == "exact"), n + 1)
            });
            (outcomes + 1, exact + e, scored + n)
        },
    )
}

impl Summary {
    pub fn cases(&self) -> usize {
        self.successes + self.failures + self.unresolved
    }
}

pub fn summary(o: &StylistOverview) -> Summary {
    let wardrobe: Vec<&StylistPiece> = o.wardrobe.iter().filter(|p| in_wardrobe(p)).collect();
    let current: Vec<&StylistPrediction> = o.predictions.iter().filter(|p| !p.superseded).collect();
    let (outcomes, exact, scored) = forecast_scores(current.iter().copied());
    Summary {
        pieces: wardrobe.len(),
        references: wardrobe
            .iter()
            .filter(|p| p.reference_role.is_some())
            .count(),
        untested: wardrobe.iter().filter(|p| p.fit == "untested").count(),
        body: o.profile.body.len(),
        stale_body: o.profile.body.iter().filter(|b| b.stale).count(),
        newest_body: o
            .profile
            .body
            .iter()
            .map(|b| b.latest.observed_on.clone())
            .max(),
        successes: o.history.success_total as usize,
        failures: o.history.failure_total as usize,
        unresolved: o.history.unresolved_total as usize,
        forecasts: current.len(),
        outcomes,
        exact,
        scored,
    }
}

fn wardrobe_phrase(s: &Summary) -> String {
    match (s.pieces, s.references) {
        (0, _) => "No pieces yet".to_owned(),
        (p, 0) => format!("{}, no references", plural(p, "piece", "pieces")),
        (p, r) => format!(
            "{}, {}",
            plural(p, "piece", "pieces"),
            plural(r, "reference", "references")
        ),
    }
}

fn record_phrase(s: &Summary) -> Option<String> {
    let forecasts = plural(s.forecasts, "forecast", "forecasts");
    let cases = plural(s.cases(), "fit case", "fit cases");
    match (s.forecasts, s.cases()) {
        (0, 0) => None,
        (_, 0) => Some(format!("{forecasts} on record")),
        (0, _) => Some(format!("{cases} on record")),
        _ => Some(format!("{forecasts} and {cases} on record")),
    }
}

/// Headline and lede: what needs the owner first, otherwise the wardrobe.
pub fn stylist_sentence(o: &StylistOverview, today: i64) -> (String, String) {
    if is_blank(o) {
        return (
            "No profile yet.".to_owned(),
            "The onboarding interview records body measurements and reference garments. Until it does, assistants abstain from size advice.".to_owned(),
        );
    }
    let items = attention(o, today);
    let s = summary(o);
    let wardrobe = wardrobe_phrase(&s);
    let record = record_phrase(&s);
    if items.is_empty() {
        let lede = match record {
            Some(r) => format!("Nothing is due. {}.", sentence_case(&r)),
            None => "Nothing is due.".to_owned(),
        };
        return (format!("{wardrobe}."), lede);
    }
    let mut parts = Vec::new();
    let counts: Vec<String> = AttentionKind::ORDER
        .iter()
        .filter_map(|k| {
            let n = items.iter().filter(|i| i.kind == *k).count();
            (n > 0).then(|| k.noun(n))
        })
        .collect();
    if counts.len() > 1 {
        parts.push(format!("{}.", sentence_case(&join_and(&counts))));
    }
    parts.push(match record {
        Some(r) => format!("{wardrobe}; {r}."),
        None => format!("{wardrobe}."),
    });
    (head_for(&items), parts.join(" "))
}

/// The backup panel's note: none yet, or the newest archive trails the
/// ledger. Only `real` is backed up.
pub fn backup_note(o: &StylistOverview, now: f64) -> Option<(Tone, String)> {
    if o.namespace != REAL || o.ledger_revision == 0.0 {
        return None;
    }
    let newest = o
        .archives
        .iter()
        .max_by(|a, b| a.ledger_revision.total_cmp(&b.ledger_revision))?;
    if newest.ledger_revision >= o.ledger_revision {
        return None;
    }
    let age = parse_date_ms(&newest.exported_at).map_or(f64::INFINITY, |t| now - t);
    Some((
        if age > BACKUP_GRACE_MS {
            Tone::Warn
        } else {
            Tone::Neutral
        },
        format!(
            "The newest archive is at revision {}; the ledger is at {}. StylistBackup writes one at 23 minutes past each hour after a change.",
            number_string(newest.ledger_revision),
            number_string(o.ledger_revision)
        ),
    ))
}

/// `backup` → `Backup`, `pre_purge` → `Pre-purge archive`.
pub fn archive_label(reason: &str) -> String {
    match reason {
        "backup" => "Backup".to_owned(),
        "export" => "Export".to_owned(),
        "pre_purge" => "Pre-purge archive".to_owned(),
        other => sentence_case(&words(other)),
    }
}

/// Which fit-history group the page shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HistoryTab {
    Failures,
    Successes,
    Unresolved,
}

/// Failures first when there are any, then successes, then unresolved.
pub fn default_tab(h: &StylistHistory) -> HistoryTab {
    if h.failure_total > 0.0 {
        HistoryTab::Failures
    } else if h.success_total > 0.0 {
        HistoryTab::Successes
    } else if h.unresolved_total > 0.0 {
        HistoryTab::Unresolved
    } else {
        HistoryTab::Failures
    }
}

fn tab_cases(h: &StylistHistory, tab: HistoryTab) -> (Vec<StylistCase>, usize) {
    match tab {
        HistoryTab::Failures => (h.failures.clone(), h.failure_total as usize),
        HistoryTab::Successes => (h.successes.clone(), h.success_total as usize),
        HistoryTab::Unresolved => (h.unresolved.clone(), h.unresolved_total as usize),
    }
}

// ----- small views -----

/// A certainty marker; explicit facts need none.
fn certainty_tag(certainty: &str) -> Option<AnyView> {
    let title = match certainty {
        "inferred" => "Derived by an assistant from other evidence",
        "unknown" => "Recorded as an open question",
        _ => return None,
    };
    let word = certainty.to_owned();
    Some(view! { <Tag title=title>{word}</Tag> }.into_any())
}

/// Regions with a verdict each: `chest ● good`, `length ◆ slightly short`.
#[component]
fn FitChips(regions: Vec<StylistRegion>, #[prop(into)] label: String) -> impl IntoView {
    view! {
        <ul class="fit-chips" aria-label=label>
            {regions
                .into_iter()
                .map(|r| {
                    let verdict = words(&r.verdict);
                    view! {
                        <li class="fit-chip">
                            <span class="fit-region">{words(&r.region)}</span>
                            <Status kind=verdict_kind(&r.verdict) label=verdict/>
                        </li>
                    }
                })
                .collect_view()}
        </ul>
    }
}

/// Region scores of one outcome: `chest ● exact`, with what was expected
/// and reported in the tooltip.
#[component]
fn ScoreChips(scores: Vec<StylistScore>) -> impl IntoView {
    view! {
        <ul class="fit-chips" aria-label="Region scores">
            {scores
                .into_iter()
                .map(|s| {
                    let said = |v: &Option<String>| v.as_deref().map_or_else(|| "nothing".to_owned(), words);
                    let title = format!("Expected {}, reported {}", said(&s.expected), said(&s.actual));
                    view! {
                        <li class="fit-chip" title=title>
                            <span class="fit-region">{words(&s.region)}</span>
                            <Status kind=match_kind(&s.matched) label=words(&s.matched)/>
                        </li>
                    }
                })
                .collect_view()}
        </ul>
    }
}

/// Measurements as a compact list: dimension (or date) and value in cm.
#[component]
fn MeasureList(
    list: Vec<StylistMeasurement>,
    today: i64,
    /// Name each dimension (garments); otherwise the date leads (earlier
    /// readings of one dimension).
    #[prop(optional)]
    with_dimension: bool,
) -> impl IntoView {
    view! {
        <ul class="measure-list">
            {list
                .into_iter()
                .map(|m| {
                    let date = date_label(&m.observed_on, today);
                    let key = if with_dimension {
                        format!("{} · {}", sentence_case(&words(&m.dimension)), words(&m.method))
                    } else {
                        date.clone()
                    };
                    let mut meta = Vec::new();
                    if recorded_differs(&m.original, m.value_mm) {
                        meta.push(m.original.clone());
                    }
                    if let Some(wash) = m.wash.as_deref().filter(|w| *w != "unknown") {
                        meta.push(words(wash));
                    }
                    if with_dimension {
                        meta.push(date);
                    }
                    view! {
                        <li>
                            <span class="measure-list-k" title=m.endpoints.clone()>{key}</span>
                            <span class="measure-list-meta">{meta.join(" · ")}</span>
                            <span class="measure-list-v num">{cm(m.value_mm)} <small>"cm"</small></span>
                        </li>
                    }
                })
                .collect_view()}
        </ul>
    }
}

// ----- sections -----

/// The namespace picker: the owner's ledger first, then test fixtures.
#[component]
fn NamespaceSwitch(ns: RwSignal<String>, namespaces: Memo<Vec<StylistNamespace>>) -> impl IntoView {
    let options = move || {
        let current = ns.get();
        let mut list = namespaces.get();
        if !list.iter().any(|n| n.namespace == current) {
            list.push(StylistNamespace {
                fixture: current != REAL,
                namespace: current.clone(),
                revision: 0.0,
            });
        }
        let option = |n: &StylistNamespace| {
            let label = if n.namespace == REAL {
                format!("Real ledger · r{}", number_string(n.revision))
            } else {
                format!("{} · r{}", n.namespace, number_string(n.revision))
            };
            view! { <option value=n.namespace.clone() selected=n.namespace == current>{label}</option> }
        };
        let real: Vec<_> = list.iter().filter(|n| !n.fixture).map(option).collect();
        let fixtures: Vec<_> = list.iter().filter(|n| n.fixture).map(option).collect();
        view! {
            {real}
            {(!fixtures.is_empty()).then(|| view! { <optgroup label="Test fixtures">{fixtures}</optgroup> })}
        }
    };
    let on_change = move |ev| {
        let value = event_target_value(&ev);
        if valid_namespace(&value) {
            replace_query_param("ns", (value != REAL).then_some(value.as_str()));
            ns.set(value);
        }
    };
    view! {
        <label class="stylist-ns">
            <span class="label">"Ledger"</span>
            <select class="select" prop:value=move || ns.get() on:change=on_change>
                {options}
            </select>
        </label>
    }
}

/// Runbook version, ledger revision and the test-data marker under the title.
#[component]
fn HeadMeta(
    data: RwSignal<Option<StylistOverview>>,
    /// A failed re-read of the namespace already shown.
    failed: Signal<Option<String>>,
) -> impl IntoView {
    let meta = Memo::new(move |_| {
        data.with(|d| {
            d.as_ref().map(|d| {
                (
                    d.namespace != REAL,
                    d.runbook.version.clone(),
                    d.ledger_revision,
                )
            })
        })
    });
    move || {
        meta.get().map(|(fixture, version, revision)| {
            view! {
                <p class="page-status stylist-meta">
                    {fixture.then(|| view! {
                        <Tag tone=Tone::Info title="Synthetic test data from a fixture namespace, not the owner's ledger">"Test data"</Tag>
                    })}
                    <span class="small muted">
                        "Runbook " <span class="mono">{version}</span>
                        " · ledger revision " <span class="num">{number_string(revision)}</span>
                    </span>
                    {move || failed.get().map(|e| view! {
                        <span class="small text-warn" title=e>"Refresh failed; showing the last read"</span>
                    })}
                </p>
            }
        })
    }
}

/// Due items, blocking warnings, contradictions and ledger checks.
#[component]
fn NeedsYou(items: Memo<Vec<Attention>>) -> impl IntoView {
    move || {
        let list = items.get();
        (!list.is_empty()).then(|| {
            view! {
                <Panel title=format!("Needs you · {}", list.len()) class="stylist-attention">
                    <div class="rows">
                        {list
                            .into_iter()
                            .map(|item| {
                                view! {
                                    <div class="row attention-row">
                                        <Status kind=item.status label=item.word/>
                                        <span class="row-main">
                                            <span class="attention-title">{item.title}</span>
                                            {item.detail.map(|d| view! { <span class="attention-detail">{d}</span> })}
                                        </span>
                                        {item.when.map(|(tone, text)| view! {
                                            <span class=format!("row-end attention-when num {}", tone_text(tone))>{text}</span>
                                        })}
                                    </div>
                                }
                            })
                            .collect_view()}
                    </div>
                </Panel>
            }
        })
    }
}

/// Pieces, body measurements, fit cases and forecasts as readouts.
#[component]
fn SummaryBand(summary: Memo<Summary>, today: Memo<i64>) -> impl IntoView {
    let figure = move |f: fn(&Summary) -> usize| {
        Signal::derive(move || summary.with(|s| number_string(f(s) as f64)))
    };
    view! {
        <ReadoutBand cols=4 aria_label="Ledger summary" class="stylist-band">
            <Readout label="Pieces" value=figure(|s| s.pieces)>
                <span>{move || summary.with(|s| {
                    let mut parts = vec![match s.references {
                        0 => "no references".to_owned(),
                        n => plural(n, "reference", "references"),
                    }];
                    if s.untested > 0 {
                        parts.push(format!("{} untested", s.untested));
                    }
                    parts.join(" · ")
                })}</span>
            </Readout>
            <Readout label="Body measurements" value=figure(|s| s.body)>
                {move || summary.with(|s| match (s.stale_body, &s.newest_body) {
                    (0, Some(newest)) => view! { <span>{format!("newest {}", date_label(newest, today.get()))}</span> }.into_any(),
                    (0, None) => view! { <span>"none recorded"</span> }.into_any(),
                    (n, _) => view! { <span class="text-warn">{format!("{n} stale")}</span> }.into_any(),
                })}
            </Readout>
            <Readout label="Fit cases" value=figure(Summary::cases)>
                <span>{move || summary.with(|s| format!("{} worked · {} failed · {} open", s.successes, s.failures, s.unresolved))}</span>
            </Readout>
            <Readout label="Forecasts" value=figure(|s| s.forecasts)>
                <span>{move || summary.with(|s| forecast_scores_phrase(s.outcomes, s.exact, s.scored))}</span>
            </Readout>
        </ReadoutBand>
    }
}

/// One body dimension: newest reading, how it was taken, earlier readings.
#[component]
fn BodyRow(entry: StylistBodyMeasurement, today: i64) -> impl IntoView {
    let StylistBodyMeasurement {
        latest: m,
        certainty,
        source,
        earlier,
        stale,
    } = entry;
    let mut sub = vec![sentence_case(&words(&m.method))];
    if !m.endpoints.trim().is_empty() {
        sub.push(m.endpoints.trim().to_owned());
    }
    if !source.trim().is_empty() {
        sub.push(words(&source));
    }
    let sub = sub.join(" · ");
    let date = date_label(&m.observed_on, today);
    let original = recorded_differs(&m.original, m.value_mm).then(|| m.original.clone());
    let count = earlier.len();
    view! {
        <article class="measure">
            <div class="row measure-row">
                <span class="row-main">
                    <span class="row-title">{sentence_case(&words(&m.dimension))}</span>
                    <span class="row-sub" title=sub.clone()>{sub.clone()}</span>
                </span>
                <span class="measure-meta">
                    {original.map(|o| view! { <span title="As recorded">{o}</span> })}
                    {if stale {
                        view! { <Status kind=StatusKind::Stale label=format!("Stale · {date}") title="Measured long enough ago to re-check"/> }.into_any()
                    } else {
                        view! { <span>{date}</span> }.into_any()
                    }}
                    {certainty_tag(&certainty)}
                </span>
                <span class="measure-value">
                    <span class="num">{cm(m.value_mm)}</span>
                    <small>"cm"</small>
                </span>
            </div>
            {(count > 0).then(|| view! {
                <Disclosure summary=plural(count, "earlier reading", "earlier readings") class="measure-earlier">
                    <MeasureList list=earlier today/>
                </Disclosure>
            })}
        </article>
    }
}

/// Profile attributes, fit sensitivities and body measurements.
#[component]
fn BodyPanel(profile: StylistProfile, notes: Vec<StylistWarning>, today: i64) -> impl IntoView {
    let snapshot_stale = notes.iter().any(|n| n.code == "profile_stale");
    let observed = profile.observed_on.clone();
    let StylistProfile {
        attributes,
        sensitivities,
        body,
        ..
    } = profile;
    let has_profile = !attributes.is_empty() || !sensitivities.is_empty();
    let note_count = notes.len();
    let meta = ViewFn::from(move || {
        observed.clone().map(|d| {
            let text = format!("snapshot {}", date_label(&d, today));
            view! { <span class=if snapshot_stale { "num text-warn" } else { "num" }>{text}</span> }
        })
    });
    view! {
        <Panel title="Body" head_end=meta class="stylist-body">
            {has_profile.then(|| view! {
                <div class="panel-body stylist-profile">
                    {(!attributes.is_empty()).then(|| view! {
                        <dl class="stylist-attrs">
                            {attributes.into_iter().map(|a| {
                                let numeric = a.value.starts_with(|c: char| c.is_ascii_digit());
                                view! {
                                    <div>
                                        <dt>{sentence_case(&words(&a.name))}</dt>
                                        <dd>
                                            <span class=if numeric { "num" } else { "" }>{a.value}</span>
                                            {certainty_tag(&a.certainty)}
                                        </dd>
                                    </div>
                                }
                            }).collect_view()}
                        </dl>
                    })}
                    {(!sensitivities.is_empty()).then(|| view! {
                        <div class="stylist-sens">
                            <span class="label">"Fit sensitivities"</span>
                            <ul class="stylist-notes">
                                {sensitivities.into_iter().map(|s| {
                                    let (region, note) = s.split_once(": ").map_or((None, s.clone()), |(r, n)| (Some(r.to_owned()), n.to_owned()));
                                    view! {
                                        <li>
                                            {region.map(|r| view! { <strong>{sentence_case(&words(&r))}</strong> " " })}
                                            {note}
                                        </li>
                                    }
                                }).collect_view()}
                            </ul>
                        </div>
                    })}
                </div>
            })}
            {if body.is_empty() {
                view! {
                    <EmptyState compact=true message="No body measurements yet. Onboarding takes them with the runbook's measuring protocol."/>
                }.into_any()
            } else {
                view! {
                    <div class="stylist-measures">
                        {body.into_iter().map(|entry| view! { <BodyRow entry today/> }).collect_view()}
                    </div>
                }.into_any()
            }}
            {(note_count > 0).then(|| view! {
                <Disclosure summary=format!("Notes · {note_count}") class="stylist-foot">
                    <ul class="stylist-notes">
                        {notes.into_iter().map(|n| view! { <li>{sentence_case(&n.message)}</li> }).collect_view()}
                    </ul>
                </Disclosure>
            })}
        </Panel>
    }
}

/// One piece: fit evidence, label and variant, reference and status marks,
/// and its newest garment measurements.
#[component]
fn PieceItem(piece: StylistPiece, today: i64) -> impl IntoView {
    let (kind, word) = fit_look(&piece.fit);
    let variant: Vec<String> = [
        piece.brand.clone(),
        piece.model.clone(),
        piece.size_label.clone().map(|s| format!("size {s}")),
        piece.colour.clone(),
        piece.fabric.clone(),
    ]
    .into_iter()
    .flatten()
    .filter(|s| !s.trim().is_empty())
    .collect();
    let variant = variant.join(" · ");
    let reference = piece.reference_role.as_deref().map(|role| {
        let regions: Vec<String> = piece.reference_regions.iter().map(|r| words(r)).collect();
        let title = if regions.is_empty() {
            "Sizing reference".to_owned()
        } else {
            format!("Sizing reference for {}", join_and(&regions))
        };
        (reference_label(role), title)
    });
    let status = piece_status_tag(&piece.status);
    let return_by = (piece.status == "undecided")
        .then(|| {
            piece
                .return_by
                .as_deref()
                .map(|d| return_by_label(d, today))
        })
        .flatten();
    let mut wear = Vec::new();
    if piece.washes > 0.0 {
        wear.push(plural(piece.washes as usize, "wash", "washes"));
    }
    if piece.alterations > 0.0 {
        wear.push(plural(
            piece.alterations as usize,
            "alteration",
            "alterations",
        ));
    }
    let closed = matches!(piece.status.as_str(), "returned" | "retired");
    let measurements = piece.measurements.clone();
    let count = measurements.len();
    view! {
        <article class=if closed { "piece closed" } else { "piece" } id=format!("piece-{}", piece.id)>
            <div class="row piece-row">
                <Status kind label=word/>
                <span class="row-main">
                    <span class="row-title">{piece.label.clone()}</span>
                    {(!variant.is_empty()).then(|| view! { <span class="row-sub" title=variant.clone()>{variant.clone()}</span> })}
                </span>
                <span class="row-end piece-end">
                    {reference.map(|(label, title)| view! { <Tag title=title>{label}</Tag> })}
                    {status.map(|(tone, label)| view! { <Tag tone>{label}</Tag> })}
                    {return_by.map(|(tone, text)| view! { <span class=format!("piece-meta {}", tone_text(tone))>{text}</span> })}
                    {(!wear.is_empty()).then(|| view! { <span class="piece-meta muted">{wear.join(" · ")}</span> })}
                </span>
            </div>
            {(count > 0).then(|| view! {
                <Disclosure summary=format!("Garment measurements · {count}") class="piece-more">
                    <MeasureList list=measurements today with_dimension=true/>
                </Disclosure>
            })}
        </article>
    }
}

/// The wardrobe by category; returned and retired pieces collapse.
#[component]
fn WardrobePanel(wardrobe: Vec<StylistPiece>, today: i64) -> impl IntoView {
    let (current, closed): (Vec<StylistPiece>, Vec<StylistPiece>) =
        wardrobe.into_iter().partition(in_wardrobe);
    let references = current
        .iter()
        .filter(|p| p.reference_role.is_some())
        .count();
    let title = format!("Wardrobe · {}", current.len());
    let meta = ViewFn::from(move || {
        (references > 0)
            .then(|| view! { <span>{plural(references, "reference", "references")}</span> })
    });
    let groups = group_by_category(&current);
    let closed_count = closed.len();
    view! {
        <Panel title head_end=meta class="stylist-wardrobe">
            {if groups.is_empty() {
                view! { <EmptyState compact=true message="No pieces yet. Onboarding records reference garments first, then everyday pieces."/> }.into_any()
            } else {
                groups.into_iter().map(|(category, pieces)| {
                    let label = category_label(&category);
                    let aria = label.clone();
                    let n = pieces.len();
                    view! {
                        <section class="stylist-group" aria-label=aria>
                            <h3 class="group-head"><span>{label}</span><span class="meta num end">{n}</span></h3>
                            {pieces.into_iter().map(|piece| view! { <PieceItem piece today/> }).collect_view()}
                        </section>
                    }
                }).collect_view().into_any()
            }}
            {(closed_count > 0).then(|| view! {
                <Disclosure summary=format!("Returned and retired · {closed_count}") class="stylist-foot">
                    {closed.into_iter().map(|piece| view! { <PieceItem piece today/> }).collect_view()}
                </Disclosure>
            })}
        </Panel>
    }
}

/// One fit case: subject, kind, what happened and the region verdicts.
#[component]
fn CaseRow(case: StylistCase, today: i64) -> impl IntoView {
    let category = case.category.as_deref().map(category_label);
    // Details name verdicts and stages by slug (`slightly_loose`).
    let detail =
        (!case.detail.trim().is_empty()).then(|| sentence_case(&words(case.detail.trim())));
    let regions = case.regions.clone();
    view! {
        <div class="row case-row">
            <span class="row-main">
                <span class="case-head">
                    <span class="case-subject">{case.subject.clone()}</span>
                    <Tag>{case_kind_label(&case.kind)}</Tag>
                </span>
                {category.map(|c| view! { <span class="row-sub">{c}</span> })}
                {detail.map(|d| view! { <span class="case-detail">{d}</span> })}
                {(!regions.is_empty()).then(|| view! { <FitChips regions label="Region verdicts"/> })}
            </span>
            <time class="row-end case-date num" datetime=case.occurred_on.clone()>{date_label(&case.occurred_on, today)}</time>
        </div>
    }
}

fn cases_view(cases: Vec<StylistCase>, total: usize, tab: HistoryTab, today: i64) -> AnyView {
    if cases.is_empty() {
        let message = match tab {
            HistoryTab::Failures => "No fit failures recorded.",
            HistoryTab::Successes => "No fit successes recorded yet.",
            HistoryTab::Unresolved => "Nothing unresolved.",
        };
        return view! { <EmptyState compact=true message=message/> }.into_any();
    }
    let shown = cases.len();
    let more = use_show_more(
        Signal::stored(cases),
        CASES_SHOWN,
        Signal::stored(String::new()),
    );
    view! {
        <div class="rows">
            <For
                each=move || more.visible.get().into_iter().enumerate()
                key=|(i, _)| *i
                children=move |(_, case)| view! { <CaseRow case today/> }
            />
        </div>
        {move || more.has_more.get().then(|| view! {
            <div class="stylist-more">
                <ShowMoreButton remaining=more.remaining on_click=Callback::new(move |()| more.show_more()) noun="cases"/>
            </div>
        })}
        {(total > shown).then(|| view! {
            <p class="stylist-note small muted">{format!("The newest {shown} of {total} are listed.")}</p>
        })}
    }
    .into_any()
}

/// Failures, successes and unresolved cases behind one segmented control.
#[component]
fn HistoryPanel(
    history: StylistHistory,
    tab: RwSignal<Option<HistoryTab>>,
    today: i64,
) -> impl IntoView {
    let total = history.success_total + history.failure_total + history.unresolved_total;
    if total == 0.0 {
        return view! {
            <Panel title="Fit history" class="stylist-history">
                <EmptyState compact=true message="No fit cases yet. Try-ons, wear, washes and outcomes recorded through the stylist tools land here."/>
            </Panel>
        }
        .into_any();
    }
    let fallback = default_tab(&history);
    let current = Signal::derive(move || tab.get().unwrap_or(fallback));
    let options = Signal::stored(vec![
        SegOption::new(HistoryTab::Failures, "Failures").with_count(history.failure_total as usize),
        SegOption::new(HistoryTab::Successes, "Successes")
            .with_count(history.success_total as usize),
        SegOption::new(HistoryTab::Unresolved, "Unresolved")
            .with_count(history.unresolved_total as usize),
    ]);
    let head = ViewFn::from(move || {
        view! {
            <Segmented
                options
                value=current
                on_change=Callback::new(move |t| tab.set(Some(t)))
                aria_label="Fit history"
                small=true
                fill=true
            />
        }
    });
    let history = StoredValue::new(history);
    view! {
        <Panel title="Fit history" head_end=head class="stylist-history">
            {move || {
                let t = current.get();
                let (cases, total) = history.with_value(|h| tab_cases(h, t));
                cases_view(cases, total, t, today)
            }}
        </Panel>
    }
    .into_any()
}

/// One outcome stage: result and per-region scores.
#[component]
fn OutcomeLine(outcome: StylistOutcome, today: i64) -> impl IntoView {
    let (kind, word) = result_look(&outcome.result);
    let key = format!(
        "{} · {}",
        stage_label(&outcome.stage),
        date_label(&outcome.occurred_on, today)
    );
    let scores = outcome.scores;
    view! {
        <div class="fit-line">
            <span class="fit-line-k">{key}</span>
            <Status kind label=word/>
            {(!scores.is_empty()).then(|| view! { <ScoreChips scores/> })}
        </div>
    }
}

/// A frozen forecast: decision, subject, basis, risk, expectations and how
/// each outcome stage scored.
#[component]
fn ForecastItem(prediction: StylistPrediction, today: i64) -> impl IntoView {
    let p = prediction;
    let (kind, word) = decision_look(&p.decision);
    let frozen = date_label(&p.frozen_at, today);
    let frozen_title = parse_date_ms(&p.frozen_at).map(format_absolute);
    let (basis_tone, basis) = basis_phrase(&p.basis, p.computed_basis.as_deref());
    let (risk_tone, risk) = risk_phrase(&p.return_risk);
    let category = (!p.category.is_empty()).then(|| category_label(&p.category));
    let gaps = p.evidence_gaps.clone();
    let failure = p.failure_mode.trim().to_owned();
    let details = match (failure.is_empty(), gaps.len()) {
        (false, 0) => "Likely failure".to_owned(),
        (false, n) => format!(
            "Likely failure and {}",
            plural(n, "evidence gap", "evidence gaps")
        ),
        (true, 0) => "Forecast details".to_owned(),
        (true, n) => format!("Evidence gaps · {n}"),
    };
    let basis_detail = match p.computed_basis.as_deref().filter(|c| *c != p.basis) {
        Some(c) => format!(
            "{} (computed: {})",
            basis_label(&p.basis),
            basis_label(c).to_lowercase()
        ),
        None => basis_label(&p.basis),
    };
    let runbook = p.runbook_version.clone();
    let anchor = format!("forecast-{}", p.id);
    let id = p.id.clone();
    let expectations = p.expectations.clone();
    let mut outcomes = p.outcomes.clone();
    outcomes.sort_by(|a, b| a.occurred_on.cmp(&b.occurred_on));
    let verified = if p.context_verified {
        view! {
            <span class="forecast-proof" title="The cited context was re-rendered and matched when the forecast froze">
                <Glyph icon=Icon::Check size=IconSize::Small/> "Context verified"
            </span>
        }
        .into_any()
    } else {
        view! {
            <span class="forecast-proof unverified" title="Frozen without a verified context token">"Context unverified"</span>
        }
        .into_any()
    };
    view! {
        <article class=if p.superseded { "forecast superseded" } else { "forecast" } id=anchor>
            <div class="row forecast-row">
                <Status kind label=word/>
                <span class="row-main">
                    <span class="row-title">{p.subject.clone()}</span>
                    <span class="forecast-facts">
                        {category.map(|c| view! { <span>{c}</span> })}
                        <span class=tone_class(basis_tone)>{basis}</span>
                        <span class=tone_class(risk_tone)>{risk}</span>
                    </span>
                </span>
                <span class="row-end forecast-end">
                    <span class="num muted" title=frozen_title>{format!("Frozen {frozen}")}</span>
                    {verified}
                    {p.superseded.then(|| view! { <Tag>"Superseded"</Tag> })}
                </span>
            </div>
            <div class="forecast-body">
                {(!expectations.is_empty()).then(|| view! {
                    <div class="fit-line">
                        <span class="fit-line-k">"Expected"</span>
                        <FitChips regions=expectations label="Expected fit by region"/>
                    </div>
                })}
                {if outcomes.is_empty() {
                    view! {
                        <div class="fit-line">
                            <span class="fit-line-k">"Outcome"</span>
                            <span class="small muted">"Not recorded yet"</span>
                        </div>
                    }.into_any()
                } else {
                    outcomes.into_iter().map(|outcome| view! { <OutcomeLine outcome today/> }).collect_view().into_any()
                }}
                <Disclosure summary=details flush=true class="forecast-more">
                    <dl class="kv">
                        {(!failure.is_empty()).then(|| view! { <dt>"Likely failure"</dt><dd>{sentence_case(&failure)}</dd> })}
                        {(!gaps.is_empty()).then(|| view! {
                            <dt>"Evidence gaps"</dt>
                            <dd><ul class="forecast-gaps">{gaps.into_iter().map(|g| view! { <li>{sentence_case(&g)}</li> }).collect_view()}</ul></dd>
                        })}
                        <dt>"Basis"</dt>
                        <dd>{basis_detail}</dd>
                        {runbook.map(|v| view! { <dt>"Runbook"</dt><dd class="mono">{v}</dd> })}
                        <dt>"Forecast"</dt>
                        <dd class="num muted">{id}</dd>
                    </dl>
                </Disclosure>
            </div>
        </article>
    }
}

/// Current forecasts first; superseded ones collapse.
#[component]
fn ForecastsPanel(predictions: Vec<StylistPrediction>, today: i64) -> impl IntoView {
    let (current, superseded): (Vec<StylistPrediction>, Vec<StylistPrediction>) =
        predictions.into_iter().partition(|p| !p.superseded);
    let title = format!("Forecasts · {}", current.len());
    let (outcomes, exact, scored) = forecast_scores(current.iter());
    let meta = ViewFn::from(move || {
        (outcomes > 0).then(
            || view! { <span class="num">{forecast_scores_phrase(outcomes, exact, scored)}</span> },
        )
    });
    let superseded_count = superseded.len();
    view! {
        <Panel title head_end=meta class="stylist-forecasts">
            {if current.is_empty() {
                view! { <EmptyState compact=true message="No forecasts yet. A forecast is frozen before a purchase or try-on, then scored region by region against what happened."/> }.into_any()
            } else {
                current.into_iter().map(|prediction| view! { <ForecastItem prediction today/> }).collect_view().into_any()
            }}
            {(superseded_count > 0).then(|| view! {
                <Disclosure summary=format!("Superseded · {superseded_count}") class="stylist-foot">
                    {superseded.into_iter().map(|prediction| view! { <ForecastItem prediction today/> }).collect_view()}
                </Disclosure>
            })}
        </Panel>
    }
}

fn rule_look(rule: &StylistRule) -> (StatusKind, &'static str) {
    if rule.needs_review {
        return (StatusKind::Warn, "Needs review");
    }
    match rule.status.as_str() {
        "active" => (StatusKind::Ok, "Active"),
        "proposed" => (StatusKind::Idle, "Proposed"),
        _ => (StatusKind::Idle, "Retired"),
    }
}

fn rule_row(rule: StylistRule) -> impl IntoView {
    let (kind, word) = rule_look(&rule);
    let mut cases = vec![plural(
        rule.in_scope_cases as usize,
        "case in scope",
        "cases in scope",
    )];
    if rule.unchecked_cases > 0.0 {
        cases.push(format!("{} unchecked", number_string(rule.unchecked_cases)));
    }
    view! {
        <div class="row ledger-row">
            <p class="ledger-title">{rule.statement.clone()}</p>
            <p class="ledger-meta">
                <Status kind label=word/>
                <span>{words(&rule.scope)}</span>
            </p>
            <p class="ledger-meta">{basis_label(&rule.basis)} " · " {cases.join(" · ")}</p>
        </div>
    }
}

/// Narrow learning rules; retired ones collapse.
#[component]
fn RulesPanel(rules: Vec<StylistRule>) -> impl IntoView {
    let (current, retired): (Vec<StylistRule>, Vec<StylistRule>) =
        rules.into_iter().partition(|r| r.status != "retired");
    let review = current.iter().filter(|r| r.needs_review).count();
    let title = format!("Rules · {}", current.len());
    let meta = ViewFn::from(move || {
        (review > 0)
            .then(|| view! { <span class="text-warn">{format!("{review} to review")}</span> })
    });
    let retired_count = retired.len();
    view! {
        <Panel title head_end=meta class="stylist-rules">
            <div class="rows">{current.into_iter().map(rule_row).collect_view()}</div>
            {(retired_count > 0).then(|| view! {
                <Disclosure summary=format!("Retired · {retired_count}") class="stylist-foot">
                    <div class="rows">{retired.into_iter().map(rule_row).collect_view()}</div>
                </Disclosure>
            })}
        </Panel>
    }
}

/// Current preferences, with the earlier statement when one changed.
#[component]
fn PreferencesPanel(preferences: Vec<StylistPreference>, today: i64) -> impl IntoView {
    let title = format!("Preferences · {}", preferences.len());
    view! {
        <Panel title class="stylist-prefs">
            <div class="rows">
                {preferences.into_iter().map(|p| {
                    let mut meta = vec![category_label(&p.category), words(&p.aspect)];
                    if let Some(region) = p.region.as_deref() {
                        meta.push(words(region));
                    }
                    meta.push(format!("since {}", date_label(&p.valid_from, today)));
                    view! {
                        <div class="row ledger-row">
                            <p class="ledger-title">{p.statement.clone()}</p>
                            <p class="ledger-meta">
                                <Tag>{p.strength.clone()}</Tag>
                                <span>{meta.join(" · ")}</span>
                            </p>
                            {p.previous.clone().map(|before| view! {
                                <p class="ledger-before"><span class="label">"Before "</span>{before}</p>
                            })}
                        </div>
                    }
                }).collect_view()}
            </div>
        </Panel>
    }
}

fn need_row(need: StylistNeed, today: i64) -> impl IntoView {
    let mut meta = vec![
        sentence_case(&words(&need.reason)),
        category_label(&need.category),
    ];
    if let Some(after) = need.review_after.as_deref() {
        meta.push(format!("review after {}", date_label(after, today)));
    }
    let status = (need.status != "open").then(|| sentence_case(&words(&need.status)));
    view! {
        <div class="row ledger-row">
            <p class="ledger-title">{need.description.clone()}</p>
            <p class="ledger-meta">
                {(need.priority == "high").then(|| view! { <Tag>"High priority"</Tag> })}
                {status.map(|s| view! { <Tag>{s}</Tag> })}
                <span>{meta.join(" · ")}</span>
            </p>
        </div>
    }
}

/// Open and paused wardrobe needs; satisfied and dropped ones collapse.
#[component]
fn NeedsPanel(needs: Vec<StylistNeed>, today: i64) -> impl IntoView {
    let (open, closed): (Vec<StylistNeed>, Vec<StylistNeed>) = needs
        .into_iter()
        .partition(|n| matches!(n.status.as_str(), "open" | "paused"));
    let title = format!("Wardrobe needs · {}", open.len());
    let closed_count = closed.len();
    view! {
        <Panel title class="stylist-needs">
            {if open.is_empty() {
                view! { <EmptyState compact=true message="No open needs."/> }.into_any()
            } else {
                view! { <div class="rows">{open.into_iter().map(|n| need_row(n, today)).collect_view()}</div> }.into_any()
            }}
            {(closed_count > 0).then(|| view! {
                <Disclosure summary=format!("Closed · {closed_count}") class="stylist-foot">
                    <div class="rows">{closed.into_iter().map(|n| need_row(n, today)).collect_view()}</div>
                </Disclosure>
            })}
        </Panel>
    }
}

fn archive_row(a: StylistArchive, now: f64) -> impl IntoView {
    let at = parse_date_ms(&a.exported_at);
    let mut sub = vec![archive_label(&a.reason), format_bytes(a.file_bytes)];
    if a.evidence > 0.0 {
        let files = plural(a.evidence as usize, "evidence file", "evidence files");
        sub.push(if a.includes_bytes {
            format!("{files} with bytes")
        } else {
            format!("{files}, digests only")
        });
    }
    view! {
        <div class="row dense archive-row" title=a.archive_id.clone()>
            <span class="row-main">
                <span class="row-title">"Revision " <span class="num">{number_string(a.ledger_revision)}</span></span>
                <span class="row-sub">{sub.join(" · ")}</span>
            </span>
            <span class="row-end num small muted" title=at.map(|t| format_relative_at(t, now))>
                {at.map_or_else(|| a.exported_at.clone(), format_absolute)}
            </span>
        </div>
    }
}

/// Stored archives of this namespace, newest first.
#[component]
fn BackupsPanel(overview: StylistOverview) -> impl IntoView {
    let now = now_ms();
    let note = backup_note(&overview, now);
    let fixture = overview.namespace != REAL;
    let mut archives = overview.archives;
    archives.sort_by(|a, b| b.exported_at.cmp(&a.exported_at));
    let newest = archives
        .first()
        .and_then(|a| parse_date_ms(&a.exported_at))
        .map(|t| format!("newest {}", format_relative_at(t, now)));
    let meta = ViewFn::from(move || {
        newest
            .clone()
            .map(|n| view! { <span class="num">{n}</span> })
    });
    let rest = archives.split_off(archives.len().min(ARCHIVES_SHOWN));
    let rest_count = rest.len();
    let empty = if fixture {
        "No archives of this fixture. Exports and pre-purge archives show here."
    } else if overview.ledger_revision == 0.0 {
        "Nothing to back up yet."
    } else {
        "No backup yet. StylistBackup writes one at 23 minutes past the hour after a change."
    };
    view! {
        <Panel title="Backups" head_end=meta class="stylist-backups">
            {note.map(|(tone, text)| view! { <p class=format!("stylist-note small {}", tone_text(tone))>{text}</p> })}
            {if archives.is_empty() {
                view! { <EmptyState compact=true message=empty/> }.into_any()
            } else {
                view! { <div class="rows dense">{archives.into_iter().map(|a| archive_row(a, now)).collect_view()}</div> }.into_any()
            }}
            {(rest_count > 0).then(|| view! {
                <Disclosure summary=format!("{} older", plural(rest_count, "archive", "archives")) class="stylist-foot">
                    <div class="rows dense">{rest.into_iter().map(|a| archive_row(a, now)).collect_view()}</div>
                </Disclosure>
            })}
        </Panel>
    }
}

/// The runbook version and its session checklist.
#[component]
fn RunbookPanel(runbook: StylistRunbook, #[prop(optional)] open: bool) -> impl IntoView {
    let version = runbook.version.clone();
    let meta = ViewFn::from(move || view! { <span class="mono">{version.clone()}</span> });
    let steps = runbook.checklist.len();
    let digest: String = runbook.sha256.chars().take(12).collect();
    view! {
        <Panel title="Runbook" head_end=meta class="stylist-runbook">
            <Disclosure summary=format!("Session checklist · {}", plural(steps, "step", "steps")) open=open>
                <ol class="stylist-checklist">
                    {runbook.checklist.into_iter().map(|step| view! { <li>{step}</li> }).collect_view()}
                </ol>
                {(!digest.is_empty()).then(|| view! {
                    <p class="stylist-digest small muted">"Published text digest " <span class="mono">{digest}"…"</span></p>
                })}
            </Disclosure>
        </Panel>
    }
}

// ----- page -----

/// What the page shows; `switch` is whether there is more than one
/// namespace to pick from, and `fixture` whether the loaded ledger (not the
/// one being requested) is a test fixture.
#[derive(Clone, Debug, PartialEq)]
pub enum PageState {
    Error(String),
    Loading,
    Blank { switch: bool, fixture: bool },
    Loaded { switch: bool },
}

/// The page state for the loaded overview, the last load error and whether
/// a namespace switch is offered.
pub fn page_state(
    data: Option<&StylistOverview>,
    error: Option<String>,
    switch: bool,
) -> PageState {
    match data {
        Some(d) if is_blank(d) => PageState::Blank {
            switch,
            fixture: d.namespace != REAL,
        },
        Some(_) => PageState::Loaded { switch },
        None => error.map_or(PageState::Loading, PageState::Error),
    }
}

/// The empty-ledger panel's title and message.
pub fn blank_copy(fixture: bool) -> (&'static str, &'static str) {
    if fixture {
        (
            "This fixture is empty",
            "Test runs write synthetic records here through the stylist tools; it holds nothing yet.",
        )
    } else {
        (
            "No profile yet",
            "The onboarding interview records body measurements and reference garments. Until it does, assistants abstain from size advice. Nothing here is seeded or guessed.",
        )
    }
}

/// A panel's input paired with the day it is judged on, so the panel
/// rebuilds when either its data or the day changes.
pub fn with_day<T>(slice: Memo<Option<T>>, today: Memo<i64>) -> Memo<Option<(T, i64)>>
where
    T: Clone + PartialEq + Send + Sync + 'static,
{
    Memo::new(move |_| slice.get().map(|value| (value, today.get())))
}

#[component]
pub fn StylistPage() -> impl IntoView {
    let requested = query_param("ns");
    let initial = requested
        .clone()
        .filter(|ns| valid_namespace(ns))
        .unwrap_or_else(|| REAL.to_owned());
    if requested.is_some_and(|ns| !valid_namespace(&ns) || ns == REAL) {
        replace_query_param("ns", None);
    }
    let ns = RwSignal::new(initial);
    let data = RwSignal::new(None::<StylistOverview>);
    let error = RwSignal::new(None::<String>);
    let tab = RwSignal::new(None::<HistoryTab>);

    let reload = use_visible_poll(
        ns.into(),
        move || {
            let wanted = ns.get_untracked();
            async move { api::fetch_stylist(&wanted).await.map(|o| (wanted, o)) }
        },
        move |(wanted, overview): (String, StylistOverview)| {
            if ns.get_untracked() == wanted {
                if data.with_untracked(|d| d.as_ref().is_some_and(|d| d.namespace != wanted)) {
                    tab.set(None);
                }
                data.set(Some(overview));
                error.set(None);
            }
        },
        move |e: ApiClientError| error.set(Some(e.message().to_owned())),
        POLL,
    );

    // A namespace switch keeps the previous ledger visible, dimmed, until
    // the new one arrives.
    let switching =
        Memo::new(move |_| data.with(|d| d.as_ref().is_some_and(|d| d.namespace != ns.get())));
    let refresh_failed = Signal::derive(move || error.get().filter(|_| !switching.get()));
    let namespaces = Memo::new(move |_| {
        data.with(|d| d.as_ref().map(|d| d.namespaces.clone()).unwrap_or_default())
    });
    let has_switch = move || namespaces.with(Vec::len) > 1 || ns.get() != REAL;
    let state = Memo::new(move |_| {
        let switch = has_switch();
        data.with(|d| page_state(d.as_ref(), error.get(), switch))
    });
    let today = Memo::new(move |_| {
        data.with(|d| d.as_ref().and_then(|d| parse_ymd(&d.today)))
            .unwrap_or_else(|| local_day_index(now_ms()))
    });
    let sentence =
        Memo::new(move |_| data.with(|d| d.as_ref().map(|d| stylist_sentence(d, today.get()))));
    let head = Signal::derive(move || sentence.get().map_or_else(|| "Stylist".to_owned(), |s| s.0));
    let lede = Signal::derive(move || sentence.get().map(|s| s.1));
    let items = Memo::new(move |_| {
        data.with(|d| {
            d.as_ref()
                .map(|d| attention(d, today.get()))
                .unwrap_or_default()
        })
    });
    let figures = Memo::new(move |_| data.with(|d| d.as_ref().map(summary).unwrap_or_default()));
    // Each section rebuilds only when its own slice changes.
    let body = Memo::new(move |_| data.with(|d| d.as_ref().map(|d| (d.profile.clone(), notes(d)))));
    let wardrobe = Memo::new(move |_| data.with(|d| d.as_ref().map(|d| d.wardrobe.clone())));
    let history = Memo::new(move |_| data.with(|d| d.as_ref().map(|d| d.history.clone())));
    let predictions = Memo::new(move |_| data.with(|d| d.as_ref().map(|d| d.predictions.clone())));
    let rules =
        Memo::new(move |_| data.with(|d| d.as_ref().map(|d| d.rules.clone()).unwrap_or_default()));
    let preferences = Memo::new(move |_| data.with(|d| d.as_ref().map(|d| d.preferences.clone())));
    let needs = Memo::new(move |_| data.with(|d| d.as_ref().map(|d| d.needs.clone())));
    // Panels that judge dates rebuild when the day changes, not only when
    // their data does.
    let body = with_day(body, today);
    let wardrobe = with_day(wardrobe, today);
    let history = with_day(history, today);
    let predictions = with_day(predictions, today);
    let preferences = with_day(preferences, today);
    let needs = with_day(needs, today);
    let backups = Memo::new(move |_| {
        data.with(|d| {
            d.as_ref().map(|d| StylistOverview {
                // Only what the backups panel reads; the rest stays empty so
                // unrelated changes do not rebuild it.
                namespace: d.namespace.clone(),
                ledger_revision: d.ledger_revision,
                archives: d.archives.clone(),
                ..blank_overview()
            })
        })
    });
    let runbook = Memo::new(move |_| data.with(|d| d.as_ref().map(|d| d.runbook.clone())));

    let heading =
        move |title: Signal<String>, lede: Signal<Option<String>>, sentence: bool, switch: bool| {
            if switch {
                view! {
                    <PageHead
                        title
                        lede
                        sentence
                        actions=ViewFn::from(move || view! { <NamespaceSwitch ns namespaces/> })
                    >
                        <HeadMeta data failed=refresh_failed/>
                    </PageHead>
                }
                .into_any()
            } else {
                view! {
                    <PageHead title lede sentence>
                        <HeadMeta data failed=refresh_failed/>
                    </PageHead>
                }
                .into_any()
            }
        };
    let progress = move || {
        switching
            .get()
            .then(|| view! { <div class="progress-line" role="presentation"></div> })
    };
    let switch_error = move || {
        let e = error.get()?;
        switching.get().then(|| {
            view! {
                <ErrorState
                    warn=true
                    title=format!("{} could not load", ns.get())
                    detail="The ledger below is the one shown before."
                    raw=e
                    retry=Callback::new(move |()| reload.run(()))
                />
            }
        })
    };
    let page_class = move || {
        if switching.get() {
            "stylist-page switching"
        } else {
            "stylist-page"
        }
    };

    move || {
        match state.get() {
        PageState::Error(e) => view! {
            <ErrorState
                title="The stylist ledger could not load"
                raw=e
                retry=Callback::new(move |()| reload.run(()))
                page=true
            />
        }
        .into_any(),
        PageState::Loading => view! {
            <div class="stack-lg" aria-busy="true">
                <PageHead title="Stylist"/>
                <SkeletonRows count=6/>
            </div>
        }
        .into_any(),
        PageState::Blank { switch, fixture } => {
            let (title, message) = blank_copy(fixture);
            view! {
                {heading(Signal::stored("Stylist".to_owned()), Signal::stored(None), false, switch)}
                <div class=page_class>
                    {progress}
                    {switch_error}
                    <Panel class="stylist-empty">
                        <EmptyState icon=Icon::Hanger title=title message=message/>
                    </Panel>
                    <div class="grid-2">
                        {move || runbook.get().map(|runbook| view! { <RunbookPanel runbook open=true/> })}
                        {move || backups.get().map(|overview| view! { <BackupsPanel overview/> })}
                    </div>
                </div>
            }
            .into_any()
        }
        PageState::Loaded { switch } => view! {
            {heading(head, lede, true, switch)}
            <div class=page_class>
                {progress}
                {switch_error}
                <NeedsYou items/>
                <SummaryBand summary=figures today/>
                <div class="split stylist-layout">
                    <div class="stylist-col">
                        {move || body.get().map(|((profile, notes), today)| view! { <BodyPanel profile notes today/> })}
                        {move || wardrobe.get().map(|(wardrobe, today)| view! { <WardrobePanel wardrobe today/> })}
                        {move || history.get().map(|(history, today)| view! { <HistoryPanel history tab today/> })}
                        {move || predictions.get().map(|(predictions, today)| view! { <ForecastsPanel predictions today/> })}
                    </div>
                    <div class="stylist-col">
                        {move || {
                            let list = rules.get();
                            (!list.is_empty()).then(|| view! { <RulesPanel rules=list/> })
                        }}
                        {move || {
                            preferences
                                .get()
                                .filter(|(list, _)| !list.is_empty())
                                .map(|(list, today)| view! { <PreferencesPanel preferences=list today/> })
                        }}
                        {move || {
                            needs
                                .get()
                                .filter(|(list, _)| !list.is_empty())
                                .map(|(list, today)| view! { <NeedsPanel needs=list today/> })
                        }}
                        {move || backups.get().map(|overview| view! { <BackupsPanel overview/> })}
                        {move || runbook.get().map(|runbook| view! { <RunbookPanel runbook/> })}
                    </div>
                </div>
            </div>
        }
        .into_any(),
    }
    }
}

/// An empty overview (the backups panel's narrowed input).
fn blank_overview() -> StylistOverview {
    StylistOverview {
        namespace: String::new(),
        namespaces: Vec::new(),
        ledger_revision: 0.0,
        generated_at: String::new(),
        today: String::new(),
        runbook: StylistRunbook {
            version: String::new(),
            sha256: String::new(),
            checklist: Vec::new(),
        },
        counts: omni_api::stylist::StylistCounts::default(),
        warnings: Vec::new(),
        due: Vec::new(),
        contradictions: Vec::new(),
        profile: StylistProfile {
            observed_on: None,
            attributes: Vec::new(),
            sensitivities: Vec::new(),
            body: Vec::new(),
        },
        wardrobe: Vec::new(),
        history: StylistHistory {
            successes: Vec::new(),
            failures: Vec::new(),
            unresolved: Vec::new(),
            success_total: 0.0,
            failure_total: 0.0,
            unresolved_total: 0.0,
        },
        predictions: Vec::new(),
        rules: Vec::new(),
        preferences: Vec::new(),
        needs: Vec::new(),
        archives: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use omni_api::stylist::{
        StylistAttribute, StylistContradiction, StylistDue, StylistOutcome, StylistScore,
    };

    use super::*;

    fn today() -> i64 {
        parse_ymd("2026-10-12").unwrap()
    }

    fn measurement(dimension: &str, value_mm: f64, original: &str) -> StylistMeasurement {
        StylistMeasurement {
            id: format!("ms_{dimension}"),
            dimension: dimension.into(),
            method: "circumference".into(),
            value_mm,
            original: original.into(),
            observed_on: "2026-10-01".into(),
            endpoints: String::new(),
            wash: None,
        }
    }

    fn piece(label: &str, category: &str, status: &str, fit: &str) -> StylistPiece {
        StylistPiece {
            id: format!("pc_{label}"),
            label: label.into(),
            category: category.into(),
            status: status.into(),
            brand: Some("Uniqlo".into()),
            model: Some("U Crew Neck T-Shirt".into()),
            size_label: Some("M".into()),
            colour: None,
            fabric: None,
            fit: fit.into(),
            reference_role: None,
            reference_regions: Vec::new(),
            washes: 0.0,
            alterations: 0.0,
            measurements: Vec::new(),
            last_event_on: None,
            return_by: None,
        }
    }

    fn due(kind: &str, days_left: Option<f64>, due_on: Option<&str>) -> StylistDue {
        StylistDue {
            kind: kind.into(),
            id: "pc_1".into(),
            label: "Navy tee".into(),
            due_on: due_on.map(str::to_owned),
            days_left,
            detail: "undecided: decide keep or return".into(),
        }
    }

    fn scored(matches: &[&str], superseded: bool) -> StylistPrediction {
        StylistPrediction {
            id: "pred_1".into(),
            frozen_at: "2026-10-01T10:00:00.000Z".into(),
            subject: "Uniqlo U Crew Neck T-Shirt, M".into(),
            category: "tops/tshirt".into(),
            decision: "recommend".into(),
            basis: "exact_repeat".into(),
            computed_basis: Some("exact_repeat".into()),
            return_risk: "low".into(),
            failure_mode: String::new(),
            expectations: Vec::new(),
            evidence_gaps: Vec::new(),
            context_verified: true,
            runbook_version: Some("fit-first-v2".into()),
            outcomes: vec![StylistOutcome {
                stage: "immediate".into(),
                result: "matched".into(),
                occurred_on: "2026-10-05".into(),
                scores: matches
                    .iter()
                    .map(|m| StylistScore {
                        region: "chest".into(),
                        expected: Some("good".into()),
                        actual: Some("good".into()),
                        matched: (*m).into(),
                    })
                    .collect(),
            }],
            superseded,
        }
    }

    fn overview() -> StylistOverview {
        StylistOverview {
            namespace: REAL.into(),
            ledger_revision: 42.0,
            today: "2026-10-12".into(),
            ..blank_overview()
        }
    }

    #[test]
    fn namespaces_follow_the_server_pattern() {
        assert!(valid_namespace("real"));
        assert!(valid_namespace("fixture-acceptance-20261012a"));
        assert!(!valid_namespace("fixture-"));
        assert!(!valid_namespace("fixture--x"));
        assert!(!valid_namespace("Real"));
        assert!(!valid_namespace("prod"));
        assert!(!valid_namespace(&format!("fixture-{}", "a".repeat(41))));
    }

    #[test]
    fn measurements_show_cm_and_the_value_as_recorded() {
        assert_eq!(cm(965.0), "96.5");
        assert_eq!(cm(539.75), "54.0");
        assert!(!recorded_differs("96.5 cm", 965.0));
        assert!(!recorded_differs("97 cm", 970.0));
        assert!(recorded_differs("38 in", 965.2));
        assert!(recorded_differs("96.55 cm", 965.5));
        assert!(recorded_differs("965 mm", 965.0));
        assert_eq!(format_bytes(512.0), "512 B");
        assert_eq!(format_bytes(1536.0), "1.5 KB");
        assert_eq!(format_bytes(3.0 * 1024.0 * 1024.0), "3.0 MB");
    }

    #[test]
    fn labels_read_as_words() {
        assert_eq!(category_label("tops/tshirt"), "Tops / tshirt");
        assert_eq!(
            category_label("bottoms/dress_trousers"),
            "Bottoms / dress trousers"
        );
        assert_eq!(category_label("any"), "Any category");
        assert_eq!(case_kind_label("try_on"), "try-on");
        assert_eq!(reference_label("too_small"), "Too-small ref");
        assert_eq!(stage_label("after_wash"), "After wash");
        assert_eq!(date_label("2026-10-03", today()), "Oct 3");
        assert_eq!(
            date_label("2025-12-24T10:00:00.000Z", today()),
            "Dec 24, 2025"
        );
        assert_eq!(days_phrase(-2), "2 days overdue");
        assert_eq!(days_phrase(1), "1 day left");
        assert_eq!(archive_label("pre_purge"), "Pre-purge archive");
    }

    #[test]
    fn shapes_follow_how_far_off_a_fit_is() {
        assert_eq!(verdict_kind("good"), StatusKind::Ok);
        assert_eq!(verdict_kind("slightly_tight"), StatusKind::Warn);
        assert_eq!(verdict_kind("too_short"), StatusKind::Fault);
        assert_eq!(verdict_kind("unknown"), StatusKind::Idle);
        assert_eq!(match_kind("close"), StatusKind::Warn);
        assert_eq!(match_kind("not_reported"), StatusKind::Idle);
        assert_eq!(fit_look("failed").0, StatusKind::Fault);
        assert_eq!(fit_look("untested").0, StatusKind::Idle);
        assert_eq!(
            result_look("not_purchased"),
            (StatusKind::Idle, "Not purchased")
        );
        assert_eq!(decision_look("recommend_with_caution").0, StatusKind::Warn);
    }

    #[test]
    fn a_stronger_claimed_basis_than_computed_is_a_warning() {
        assert_eq!(
            basis_phrase("exact_repeat", Some("exact_repeat")),
            (Tone::Neutral, "Exact repeat".into())
        );
        assert_eq!(
            basis_phrase("exact_repeat", Some("known_brand_new_cut")),
            (
                Tone::Warn,
                "Claimed exact repeat · computed known brand, new cut".into()
            )
        );
        assert_eq!(
            basis_phrase("new_brand_inference", Some("known_brand_new_cut")).0,
            Tone::Neutral
        );
        assert_eq!(risk_phrase("high").0, Tone::Warn);
        assert_eq!(risk_phrase("low").1, "Low return risk");
    }

    #[test]
    fn return_deadlines_warn_late_and_fault_when_closed() {
        assert_eq!(
            return_by_label("2026-10-20", today()),
            (Tone::Neutral, "Return by Oct 20".into())
        );
        assert_eq!(return_by_label("2026-10-14", today()).0, Tone::Warn);
        assert_eq!(
            return_by_label("2026-10-12", today()),
            (Tone::Warn, "Return by today".into())
        );
        assert_eq!(
            return_by_label("2026-10-10", today()),
            (Tone::Fault, "Return window closed Oct 10".into())
        );
    }

    #[test]
    fn pieces_group_by_category_in_order() {
        let list = vec![
            piece("a", "bottoms/jeans", "owned", "proven"),
            piece("b", "tops/tshirt", "owned", "untested"),
            piece("c", "bottoms/jeans", "returned", "failed"),
        ];
        let groups = group_by_category(&list);
        let shape: Vec<(&str, usize)> = groups.iter().map(|(c, l)| (c.as_str(), l.len())).collect();
        assert_eq!(shape, [("bottoms/jeans", 2), ("tops/tshirt", 1)]);
        assert!(in_wardrobe(&list[0]));
        assert!(!in_wardrobe(&list[2]));
    }

    #[test]
    fn an_empty_ledger_reads_as_onboarding() {
        let mut o = overview();
        o.ledger_revision = 0.0;
        assert!(is_blank(&o));
        assert_eq!(stylist_sentence(&o, today()).0, "No profile yet.");
        o.archives.push(StylistArchive {
            archive_id: "a".into(),
            reason: "backup".into(),
            ledger_revision: 0.0,
            exported_at: "2026-10-12T00:00:00.000Z".into(),
            includes_bytes: false,
            file_bytes: 10.0,
            evidence: 0.0,
        });
        assert!(is_blank(&o), "archives alone are not records");
    }

    #[test]
    fn the_sentence_leads_with_what_needs_you() {
        let mut o = overview();
        let mut reference = piece("ref", "tops/tshirt", "owned", "proven");
        reference.reference_role = Some("baseline".into());
        o.wardrobe = vec![
            reference,
            piece("b", "tops/tshirt", "owned", "untested"),
            piece("c", "tops/tshirt", "returned", "failed"),
        ];
        o.predictions = vec![scored(&["exact", "close", "not_reported"], false)];
        o.history.failure_total = 1.0;
        assert_eq!(
            stylist_sentence(&o, today()),
            (
                "2 pieces, 1 reference.".into(),
                "Nothing is due. 1 forecast and 1 fit case on record.".into()
            )
        );

        o.due = vec![due("return_decision", Some(2.0), Some("2026-10-14"))];
        assert_eq!(
            stylist_sentence(&o, today()),
            (
                "1 return decision is open.".into(),
                "2 pieces, 1 reference; 1 forecast and 1 fit case on record.".into()
            )
        );

        o.contradictions = vec![StylistContradiction {
            source: "computed".into(),
            kind: "measurement".into(),
            description: "chest readings disagree by 22 mm".into(),
        }];
        o.warnings = vec![StylistWarning {
            code: "open_contradictions".into(),
            severity: "warning".into(),
            message: "1 unresolved contradictions".into(),
        }];
        let (head, lede) = stylist_sentence(&o, today());
        assert_eq!(head, "2 things need you.");
        assert!(lede.starts_with("1 return decision and 1 contradiction. "));
        // The contradiction itself is listed, not the warning that counts it.
        assert_eq!(attention(&o, today()).len(), 2);
    }

    #[test]
    fn attention_marks_deadlines_and_blocking_warnings() {
        let mut o = overview();
        o.due = vec![
            due("return_decision", Some(-1.0), Some("2026-10-11")),
            due("return_decision", Some(9.0), Some("2026-10-21")),
            due("outcome_follow_up", None, None),
            due("need_review", Some(0.0), Some("2026-10-12")),
        ];
        o.warnings = vec![
            StylistWarning {
                code: "no_fit_evidence".into(),
                severity: "blocking".into(),
                message: "no owned baseline reference".into(),
            },
            StylistWarning {
                code: "body_measurement_stale".into(),
                severity: "info".into(),
                message: "body chest was last measured on 2024-01-01".into(),
            },
        ];
        let items = attention(&o, today());
        let looks: Vec<(StatusKind, Option<String>)> = items
            .iter()
            .map(|i| (i.status, i.when.as_ref().map(|w| w.1.clone())))
            .collect();
        assert_eq!(
            looks,
            [
                (StatusKind::Fault, None),
                (StatusKind::Fault, Some("Oct 11 · 1 day overdue".into())),
                (StatusKind::Idle, Some("Oct 21 · 9 days left".into())),
                (StatusKind::Idle, None),
                (StatusKind::Idle, Some("today".into())),
            ]
        );
        assert_eq!(items[0].title, "No owned baseline reference");
        assert_eq!(
            items[1].detail.as_deref(),
            Some("Undecided: decide keep or return")
        );
        assert_eq!(notes(&o).len(), 1);
        assert_eq!(stylist_sentence(&o, today()).0, "5 things need you.");
        o.due.clear();
        o.wardrobe = vec![piece("b", "tops/tshirt", "owned", "untested")];
        assert_eq!(stylist_sentence(&o, today()).0, "Size advice is on hold.");
    }

    #[test]
    fn a_missing_profile_with_pieces_says_so() {
        let mut o = overview();
        o.wardrobe = vec![piece("b", "tops/tshirt", "owned", "untested")];
        o.warnings = vec![StylistWarning {
            code: "no_profile".into(),
            severity: "warning".into(),
            message: "no profile snapshot or body measurements are recorded".into(),
        }];
        assert!(!is_blank(&o));
        assert_eq!(stylist_sentence(&o, today()).0, "No profile yet.");
    }

    #[test]
    fn summary_counts_current_forecasts_and_scored_regions() {
        let mut o = overview();
        o.predictions = vec![
            scored(&["exact", "close", "miss", "not_forecast"], false),
            scored(&["exact"], true),
        ];
        o.profile.body = vec![StylistBodyMeasurement {
            latest: measurement("chest", 965.0, "96.5 cm"),
            certainty: "explicit".into(),
            source: "user_measurement".into(),
            earlier: Vec::new(),
            stale: true,
        }];
        o.profile.attributes = vec![StylistAttribute {
            name: "height".into(),
            value: "180 cm".into(),
            certainty: "explicit".into(),
        }];
        let s = summary(&o);
        assert_eq!((s.forecasts, s.exact, s.scored), (1, 1, 3));
        assert_eq!((s.body, s.stale_body), (1, 1));
        assert_eq!(s.newest_body.as_deref(), Some("2026-10-01"));
    }

    #[test]
    fn backups_note_only_a_real_ledger_that_moved_on() {
        let now = parse_date_ms("2026-10-12T12:00:00.000Z").unwrap();
        let archive = |revision: f64, at: &str| StylistArchive {
            archive_id: "real-r1".into(),
            reason: "backup".into(),
            ledger_revision: revision,
            exported_at: at.into(),
            includes_bytes: false,
            file_bytes: 2048.0,
            evidence: 0.0,
        };
        let mut o = overview();
        assert_eq!(backup_note(&o, now), None);
        o.archives = vec![archive(42.0, "2026-10-12T11:23:00.000Z")];
        assert_eq!(backup_note(&o, now), None);
        o.archives = vec![archive(40.0, "2026-10-12T11:23:00.000Z")];
        assert_eq!(backup_note(&o, now).map(|n| n.0), Some(Tone::Neutral));
        o.archives = vec![archive(40.0, "2026-10-12T08:23:00.000Z")];
        assert_eq!(backup_note(&o, now).map(|n| n.0), Some(Tone::Warn));
        o.namespace = "fixture-a".into();
        assert_eq!(backup_note(&o, now), None);
    }

    #[test]
    fn history_opens_on_failures_when_there_are_any() {
        let mut h = blank_overview().history;
        assert_eq!(default_tab(&h), HistoryTab::Failures);
        h.unresolved_total = 2.0;
        assert_eq!(default_tab(&h), HistoryTab::Unresolved);
        h.success_total = 1.0;
        assert_eq!(default_tab(&h), HistoryTab::Successes);
        h.failure_total = 1.0;
        assert_eq!(default_tab(&h), HistoryTab::Failures);
    }

    #[test]
    fn the_empty_state_follows_the_loaded_ledger_not_the_requested_one() {
        let real = overview();
        let fixture = StylistOverview {
            namespace: "fixture-empty".into(),
            ..overview()
        };
        let real_state = page_state(Some(&real), None, true);
        let fixture_state = page_state(Some(&fixture), None, true);
        assert_eq!(
            real_state,
            PageState::Blank {
                switch: true,
                fixture: false
            }
        );
        assert_eq!(
            fixture_state,
            PageState::Blank {
                switch: true,
                fixture: true
            }
        );
        // Switching between two empty ledgers changes the state, so the
        // copy is rebuilt.
        assert_ne!(real_state, fixture_state);
        assert_eq!(blank_copy(false).0, "No profile yet");
        assert_eq!(blank_copy(true).0, "This fixture is empty");
        assert_eq!(page_state(None, None, false), PageState::Loading);
        assert_eq!(
            page_state(None, Some("down".into()), false),
            PageState::Error("down".into())
        );
    }

    #[test]
    fn panels_rebuild_when_the_day_changes_with_unchanged_data() {
        let day = RwSignal::new(today());
        let today = Memo::new(move |_| day.get());
        let pieces =
            Memo::new(move |_| Some(vec![piece("b", "tops/tshirt", "undecided", "untested")]));
        let input = with_day(pieces, today);
        assert_eq!(input.get().map(|(_, d)| d), Some(day.get_untracked()));
        let tomorrow = day.get_untracked() + 1;
        day.set(tomorrow);
        assert_eq!(input.get().map(|(_, d)| d), Some(tomorrow));
    }

    #[test]
    fn return_deadlines_agree_across_panels_through_midnight() {
        let deadline = "2026-10-12";
        let mut o = overview();
        // The server counted the days on Oct 11; the page judges by its own day.
        o.due = vec![due("return_decision", Some(1.0), Some(deadline))];
        for (day, text, tone) in [
            ("2026-10-11", "Return by Oct 12", Tone::Warn),
            ("2026-10-12", "Return by today", Tone::Warn),
            ("2026-10-13", "Return window closed Oct 12", Tone::Fault),
        ] {
            let today = parse_ymd(day).unwrap();
            let (wardrobe_tone, wardrobe_text) = return_by_label(deadline, today);
            assert_eq!(
                (wardrobe_tone, wardrobe_text.as_str()),
                (tone, text),
                "{day}"
            );
            let needs = &attention(&o, today)[0];
            assert_eq!(needs.when.as_ref().map(|w| w.0), Some(tone), "{day}");
        }
        assert_eq!(
            attention(&o, parse_ymd("2026-10-13").unwrap())[0].status,
            StatusKind::Fault
        );
    }

    #[test]
    fn recorded_outcomes_without_region_scores_are_not_no_outcomes() {
        let mut unscored = scored(&[], false);
        unscored.outcomes[0].result = "not_purchased".into();
        let mut o = overview();
        o.predictions = vec![unscored.clone()];
        let s = summary(&o);
        assert_eq!((s.outcomes, s.exact, s.scored), (1, 0, 0));
        assert_eq!(
            forecast_scores_phrase(s.outcomes, s.exact, s.scored),
            "1 outcome, no region scored"
        );
        let mut unknown = scored(&["unknown", "not_reported"], false);
        unknown.outcomes[0].result = "unresolved".into();
        o.predictions = vec![unscored, unknown];
        let s = summary(&o);
        assert_eq!(
            forecast_scores_phrase(s.outcomes, s.exact, s.scored),
            "2 outcomes, no region scored"
        );
        let mut none = scored(&[], false);
        none.outcomes.clear();
        o.predictions = vec![none];
        let s = summary(&o);
        assert_eq!(
            forecast_scores_phrase(s.outcomes, s.exact, s.scored),
            "no outcomes yet"
        );
        o.predictions = vec![scored(&["exact", "miss"], false)];
        let s = summary(&o);
        assert_eq!(
            forecast_scores_phrase(s.outcomes, s.exact, s.scored),
            "1 of 2 regions exact"
        );
    }
}
