//! `stylist_context_get`: everything a styling session needs from the ledger,
//! assembled deterministically from one snapshot. Failures and counterexamples
//! are listed beside successes, and warnings mark missing, stale, inferred or
//! conflicting evidence.

use std::collections::{BTreeMap, BTreeSet};

use jiff::civil::Date;
use serde_json::{Map, Value, json};

use crate::api::{
    BodyMeasurementOut, BrandHistory, ContextCounts, ContextInput, ContextOutput, ContradictionOut,
    DueItem, HistoryEntry, HistoryOut, HistoryTotals, PreferenceChange, PreferencesOut, ProfileOut,
    ReferenceOut, RegionVerdict, RuleOut, RulesOut, RunbookOut, Snapshot, WarningOut,
};
use crate::error::{StylistError, not_found, reject};
use crate::fit::{self, RegionComparison, RegionMatch};
use crate::model::{
    Certainty, ChangeKind, ContradictionStatus, EventBody, EventType, FeedbackTiming,
    MeasurementRecord, NeedStatus, OutcomeResult, Rating, RecordBody, RecordKind, ReferenceRole,
    RuleStatus, SubjectKind, TryOnOverall, VariantRecord, WashState,
};
use crate::runbook;
use crate::service::{digest_json, iso, measurement_ref, record_item};
use crate::state::{Event, LedgerState, PieceStatus, Record, enum_text};
use crate::types::{Category, ContextToken, parse_date};
use crate::units;
use serde::{Deserialize, Serialize};

/// Days after which a body measurement or profile snapshot is stale.
const STALE_DAYS: i64 = 365;
/// Repeated body measurements this close in time should agree.
const AGREEMENT_WINDOW_DAYS: i64 = 90;
/// A prediction without an outcome after this long is a follow-up.
const FOLLOW_UP_DAYS: i64 = 14;

struct Focus {
    category: Option<Category>,
    brand: Option<String>,
    model: Option<String>,
}

fn same(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

fn categories_overlap(a: &Category, b: &Category) -> bool {
    a.is_under(b.as_str()) || b.is_under(a.as_str())
}

impl Focus {
    fn is_empty(&self) -> bool {
        self.category.is_none() && self.brand.is_none()
    }

    /// Relevant when the category overlaps or the brand matches.
    fn variant(&self, variant: &VariantRecord) -> bool {
        self.is_empty()
            || self
                .category
                .as_ref()
                .is_some_and(|c| categories_overlap(c, &variant.category))
            || self.brand.as_ref().is_some_and(|b| same(b, &variant.brand))
    }

    fn category(&self, category: &Category) -> bool {
        self.category
            .as_ref()
            .is_none_or(|c| categories_overlap(c, category))
    }
}

fn days_between(from: Date, to: Date) -> i64 {
    (to - from).get_days().into()
}

fn date_of(text: &str) -> Option<Date> {
    parse_date(text).ok()
}

fn day_of(ms: i64) -> String {
    iso(ms).chars().take(10).collect()
}

/// Builds the context for `input` over `state`.
pub fn build(
    state: &LedgerState,
    input: &ContextInput,
    today: Date,
    now_ms: i64,
) -> Result<ContextOutput, StylistError> {
    let runbook = match &input.runbook_version {
        Some(version) => runbook::find(version)
            .ok_or_else(|| not_found(format!("no runbook version {version}")))?,
        None => runbook::current(),
    };
    let limit = usize::try_from(input.limit.unwrap_or(15).clamp(1, 50)).unwrap_or(15);
    let requested = input.focus.clone().unwrap_or_default();
    let mut focus = Focus {
        category: requested.category.clone(),
        brand: requested.brand.clone(),
        model: None,
    };
    if let Some(id) = &requested.variant_id {
        let variant = state.variant(id.as_str()).ok_or_else(|| {
            not_found(format!("no variant {id} in namespace {}", state.namespace))
        })?;
        focus
            .category
            .get_or_insert_with(|| variant.category.clone());
        focus.brand.get_or_insert_with(|| variant.brand.clone());
        focus.model = Some(variant.model.clone());
    }
    let mut focus_echo = Map::new();
    focus_echo.insert("category".into(), json!(focus.category));
    focus_echo.insert("brand".into(), json!(focus.brand));
    focus_echo.insert("model".into(), json!(focus.model));
    focus_echo.insert("variantId".into(), json!(requested.variant_id));

    let profile = profile(state, today);
    let references = references(state, &focus);
    let history = history(state, &focus, limit);
    let brand_history = brand_history(state, &focus);
    let preferences = preferences(state, &focus, today);
    let rules = rules(state, &focus);
    let contradictions = contradictions(state);
    let due = due(state, today);
    let warnings = warnings(
        state,
        &focus,
        &requested,
        &profile,
        &references,
        &contradictions,
        &rules,
        today,
    );
    let open_needs = state
        .current_records()
        .filter(|r| match &r.body {
            RecordBody::Need(n) => n.status == NeedStatus::Open && focus.category(&n.category),
            _ => false,
        })
        .map(|r| record_item(state, r))
        .collect();
    let mut kinds = Map::new();
    for kind in RecordKind::ALL {
        let count = state.current_records().filter(|r| r.kind == kind).count();
        kinds.insert(kind.as_str().to_owned(), json!(count));
    }
    let counts = ContextCounts {
        records: kinds,
        events: u64::try_from(state.current_events().count()).unwrap_or(0),
        evidence: u64::try_from(state.evidence.len()).unwrap_or(0),
    };
    let mut output = ContextOutput {
        snapshot: Snapshot {
            namespace: state.namespace.as_str().to_owned(),
            ledger_revision: state.revision,
            newest_revision: state.current_revision,
            generated_at: iso(now_ms),
            today: today.to_string(),
            runbook_version: runbook.version.to_owned(),
            runbook_sha256: runbook.sha256().to_owned(),
            context_sha256: String::new(),
            context_token: String::new(),
        },
        runbook: RunbookOut {
            version: runbook.version.to_owned(),
            sha256: runbook.sha256().to_owned(),
            versions: runbook::versions(),
            checklist: runbook.checklist.iter().map(|s| (*s).to_owned()).collect(),
            text: input
                .include_runbook
                .unwrap_or(true)
                .then(|| runbook.text.to_owned()),
        },
        focus: focus_echo,
        profile,
        references,
        history,
        brand_history,
        preferences,
        rules,
        contradictions,
        warnings,
        due,
        open_needs,
        counts,
    };
    let digest = content_digest(&output)?;
    let params = TokenParams {
        v: 1,
        ns: state.namespace.as_str().to_owned(),
        rev: state.revision,
        today: today.to_string(),
        focus: input.focus.clone(),
        limit: input.limit.unwrap_or(15).clamp(1, 50),
        runbook: runbook.version.to_owned(),
    };
    output.snapshot.context_token = issue_token(&params, &digest)?;
    output.snapshot.context_sha256 = digest;
    Ok(output)
}

/// What a context token names.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenParams {
    pub v: u8,
    pub ns: String,
    pub rev: u64,
    pub today: String,
    pub focus: Option<crate::api::Focus>,
    pub limit: u64,
    pub runbook: String,
}

/// The digest of a context's content. Fields that change as the ledger grows
/// or as runbooks are added are excluded, so the same revision, date, focus,
/// limit and runbook version always produce the same digest.
fn content_digest(output: &ContextOutput) -> Result<String, StylistError> {
    let mut value =
        serde_json::to_value(output).map_err(|e| StylistError::Corrupt(e.to_string()))?;
    if let Some(snapshot) = value.get_mut("snapshot").and_then(Value::as_object_mut) {
        for key in [
            "generatedAt",
            "newestRevision",
            "contextSha256",
            "contextToken",
        ] {
            snapshot.shift_remove(key);
        }
    }
    if let Some(runbook) = value.get_mut("runbook").and_then(Value::as_object_mut) {
        runbook.shift_remove("text");
        runbook.shift_remove("versions");
    }
    if let Some(Value::Array(warnings)) = value.get_mut("warnings") {
        warnings.retain(|w| w.get("code") != Some(&json!("historical_snapshot")));
    }
    Ok(digest_json(&value))
}

fn issue_token(params: &TokenParams, digest: &str) -> Result<String, StylistError> {
    use base64::Engine as _;
    let value = serde_json::to_value(params).map_err(|e| StylistError::Corrupt(e.to_string()))?;
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(crate::service::canonical(&value).to_string());
    Ok(format!("ctx1.{encoded}.{digest}"))
}

fn mismatch(message: impl Into<String>) -> StylistError {
    reject("context_mismatch", message)
}

/// The parameters and digest a token carries.
pub fn parse_token(token: &ContextToken) -> Result<(TokenParams, String), StylistError> {
    use base64::Engine as _;
    let mut parts = token.as_str().split('.');
    let (Some("ctx1"), Some(encoded), Some(digest), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(mismatch(
            "the context token is malformed; copy snapshot.contextToken unchanged",
        ));
    };
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| {
            mismatch("the context token is malformed; copy snapshot.contextToken unchanged")
        })?;
    let params: TokenParams = serde_json::from_slice(&bytes).map_err(|_| {
        mismatch("the context token is malformed; copy snapshot.contextToken unchanged")
    })?;
    if params.v != 1 {
        return Err(mismatch(format!(
            "unsupported context token version {}",
            params.v
        )));
    }
    Ok((params, digest.to_owned()))
}

/// A verified link between a forecast and the context it was made from.
#[derive(Clone, Debug)]
pub struct ContextProof {
    pub params: TokenParams,
    pub sha256: String,
}

/// Re-renders the context a token names from `conn` and checks its digest.
pub fn verify_token(
    conn: &rusqlite::Connection,
    namespace: &crate::types::Namespace,
    token: &ContextToken,
) -> Result<ContextProof, StylistError> {
    let (params, digest) = parse_token(token)?;
    if params.ns != namespace.as_str() {
        return Err(mismatch(format!(
            "the context token is for namespace {}, not {namespace}",
            params.ns
        )));
    }
    let state = LedgerState::load(conn, namespace, Some(params.rev))
        .map_err(|e| mismatch(format!("the context token's revision is unusable: {e}")))?;
    let output = render(&state, &params)?;
    if output.snapshot.context_sha256 != digest {
        return Err(mismatch(format!(
            "the context token does not match the ledger at revision {}: it was altered, or the context changed shape after a deploy; read the context again",
            params.rev
        )));
    }
    Ok(ContextProof {
        params,
        sha256: digest,
    })
}

/// The context `params` name, rendered over `state` (already at `params.rev`).
fn render(state: &LedgerState, params: &TokenParams) -> Result<ContextOutput, StylistError> {
    let today = parse_date(&params.today).map_err(mismatch)?;
    let input = ContextInput {
        namespace: state.namespace.clone(),
        focus: params.focus.clone(),
        as_of_revision: Some(params.rev),
        include_runbook: Some(false),
        runbook_version: Some(params.runbook.clone()),
        limit: Some(params.limit),
        context_token: None,
    };
    build(state, &input, today, 0)
}

fn measurements(state: &LedgerState) -> impl Iterator<Item = (&Record, &MeasurementRecord)> {
    state.current_records().filter_map(|r| match &r.body {
        RecordBody::Measurement(m) => Some((r, m)),
        _ => None,
    })
}

fn profile(state: &LedgerState, today: Date) -> ProfileOut {
    let mut snapshots: Vec<&Record> = state
        .current_records()
        .filter(|r| r.kind == RecordKind::ProfileSnapshot)
        .collect();
    snapshots.sort_by_key(|r| match &r.body {
        RecordBody::ProfileSnapshot(p) => (p.observed_on.clone(), r.ledger_revision),
        _ => (String::new(), r.ledger_revision),
    });
    let mut groups: BTreeMap<(String, String), Vec<(&Record, &MeasurementRecord)>> =
        BTreeMap::new();
    for (record, m) in measurements(state).filter(|(_, m)| m.subject.kind == SubjectKind::Body) {
        groups
            .entry((m.dimension.as_str().to_owned(), enum_text(&m.method)))
            .or_default()
            .push((record, m));
    }
    let body_measurements = groups
        .into_iter()
        .filter_map(|((dimension, method), mut list)| {
            list.sort_by(|(ra, a), (rb, b)| {
                (b.observed_on.as_str(), rb.ledger_revision)
                    .cmp(&(a.observed_on.as_str(), ra.ledger_revision))
            });
            let (latest_record, latest) = *list.first()?;
            let stale =
                date_of(&latest.observed_on).is_some_and(|d| days_between(d, today) > STALE_DAYS);
            Some(BodyMeasurementOut {
                dimension,
                method,
                latest: measurement_ref(latest_record, latest),
                certainty: enum_text(&latest.provenance.certainty),
                source: enum_text(&latest.provenance.source),
                earlier: list[1..]
                    .iter()
                    .map(|(r, m)| measurement_ref(r, m))
                    .collect(),
                stale,
            })
        })
        .collect();
    ProfileOut {
        latest_snapshot: snapshots.last().map(|r| record_item(state, r)),
        snapshot_count: u64::try_from(snapshots.len()).unwrap_or(0),
        body_measurements,
    }
}

fn references(state: &LedgerState, focus: &Focus) -> Vec<ReferenceOut> {
    let mut out = Vec::new();
    for record in state.current_records() {
        let RecordBody::Piece(piece) = &record.body else {
            continue;
        };
        let Some(reference) = &piece.reference else {
            continue;
        };
        if focus.category.is_some() && !reference.categories.iter().any(|c| focus.category(c)) {
            continue;
        }
        let id = record.id.as_str();
        let mut latest: BTreeMap<(String, String), (&Record, &MeasurementRecord)> = BTreeMap::new();
        for (r, m) in measurements(state).filter(|(_, m)| {
            m.subject.kind == SubjectKind::Piece
                && m.subject.id.as_ref().is_some_and(|s| s.as_str() == id)
        }) {
            let key = (m.dimension.as_str().to_owned(), enum_text(&m.method));
            let newer = latest.get(&key).is_none_or(|(old_r, old)| {
                (m.observed_on.as_str(), r.ledger_revision)
                    > (old.observed_on.as_str(), old_r.ledger_revision)
            });
            if newer {
                latest.insert(key, (r, m));
            }
        }
        let washes: Vec<&Event> = state
            .piece_events(id)
            .filter(|e| e.event_type == EventType::Wash)
            .collect();
        let alterations = state
            .piece_events(id)
            .filter(|e| e.event_type == EventType::Alteration)
            .count();
        let measured: Vec<_> = latest.values().collect();
        out.push(ReferenceOut {
            piece: record_item(state, record),
            variant: piece
                .variant_id
                .as_ref()
                .and_then(|v| state.record(v.as_str()))
                .map(|v| record_item(state, v)),
            role: enum_text(&reference.role),
            categories: reference
                .categories
                .iter()
                .map(|c| c.as_str().to_owned())
                .collect(),
            regions: reference
                .regions
                .iter()
                .map(|r| r.as_str().to_owned())
                .collect(),
            measurements: measured
                .iter()
                .map(|(r, m)| measurement_ref(r, m))
                .collect(),
            washes: u64::try_from(washes.len()).unwrap_or(0),
            last_wash_on: washes.last().map(|e| e.occurred_on.clone()),
            washed_baseline: !measured.is_empty()
                && measured.iter().all(|(_, m)| {
                    m.garment_state
                        .as_ref()
                        .is_some_and(|s| s.wash == WashState::Washed)
                }),
            alterations: u64::try_from(alterations).unwrap_or(0),
        });
    }
    out
}

#[derive(PartialEq, Eq)]
enum Bucket {
    Success,
    Failure,
    Unresolved,
}

fn adverse_rating(rating: Option<Rating>) -> bool {
    rating == Some(Rating::Negative)
}

fn entry(
    state: &LedgerState,
    kind: &str,
    event: Option<&Event>,
    piece_id: Option<&str>,
    prediction_id: Option<&str>,
    occurred_on: &str,
    detail: String,
) -> HistoryEntry {
    let variant = piece_id
        .or(prediction_id)
        .and_then(|id| state.variant_of(id));
    let regions = event
        .map(|e| {
            e.envelope
                .event
                .region_fit()
                .iter()
                .map(|fit| RegionVerdict {
                    region: fit.region.as_str().to_owned(),
                    verdict: enum_text(&fit.verdict),
                })
                .collect()
        })
        .unwrap_or_default();
    let comparison: Vec<RegionComparison> = event
        .and_then(|e| e.derived.as_ref())
        .and_then(|d| d.get("forecastComparison"))
        .and_then(|c| serde_json::from_value(c.clone()).ok())
        .unwrap_or_default();
    HistoryEntry {
        kind: kind.to_owned(),
        occurred_on: occurred_on.to_owned(),
        event_id: event.map(|e| e.id.as_str().to_owned()),
        piece_id: piece_id.map(str::to_owned),
        prediction_id: prediction_id.map(str::to_owned),
        variant_id: variant.map(|(id, _)| id.to_owned()),
        brand: variant.map(|(_, v)| v.brand.clone()),
        model: variant.map(|(_, v)| v.model.clone()),
        size_label: variant.map(|(_, v)| v.size_label.clone()),
        category: variant
            .map(|(_, v)| v.category.as_str().to_owned())
            .or_else(|| {
                piece_id
                    .and_then(|id| state.piece(id))
                    .map(|p| p.category.as_str().to_owned())
            }),
        regions,
        detail,
        comparison,
    }
}

fn relevant(state: &LedgerState, focus: &Focus, subject: Option<&str>) -> bool {
    if focus.is_empty() {
        return true;
    }
    let Some(id) = subject else {
        return false;
    };
    if let Some((_, variant)) = state.variant_of(id) {
        return focus.variant(variant);
    }
    state.piece(id).is_some_and(|p| {
        focus
            .category
            .as_ref()
            .is_some_and(|c| categories_overlap(c, &p.category))
    })
}

fn history(state: &LedgerState, focus: &Focus, limit: usize) -> HistoryOut {
    let mut buckets: Vec<(Bucket, HistoryEntry, u64)> = Vec::new();
    let mut assessed_pieces: BTreeSet<String> = BTreeSet::new();
    let mut predictions_with_outcome: BTreeSet<String> = BTreeSet::new();
    for event in state.current_events() {
        let body = &event.envelope.event;
        let piece_id = body.piece_id().map(|id| id.as_str());
        let prediction_id = body.prediction_id().map(|id| id.as_str());
        let judged_try_on =
            matches!(body, EventBody::TryOn(t) if t.overall != TryOnOverall::Unresolved);
        if (judged_try_on || fit::known_fits(body.region_fit()).next().is_some())
            && let Some(piece) = piece_id
        {
            assessed_pieces.insert(piece.to_owned());
        }
        if let EventBody::Outcome(o) = body {
            predictions_with_outcome.insert(o.prediction_id.as_str().to_owned());
        }
        if !relevant(state, focus, piece_id.or(prediction_id)) {
            continue;
        }
        let adverse_regions: Vec<String> = body
            .region_fit()
            .iter()
            .filter(|fit| fit::is_adverse(fit.verdict))
            .map(|fit| format!("{} {}", fit.region, enum_text(&fit.verdict)))
            .collect();
        let classified = match body {
            EventBody::Outcome(o) => {
                let bucket = match o.result {
                    OutcomeResult::Matched => Bucket::Success,
                    OutcomeResult::Partial | OutcomeResult::Missed => Bucket::Failure,
                    OutcomeResult::Unresolved | OutcomeResult::NotPurchased => Bucket::Unresolved,
                };
                Some((
                    bucket,
                    "outcome",
                    format!(
                        "forecast {} at stage {}",
                        enum_text(&o.result),
                        enum_text(&o.stage)
                    ),
                ))
            }
            EventBody::TryOn(_) | EventBody::Wear(_) | EventBody::Feedback(_) => {
                let (comfort, style, quality, value, regret) = match body {
                    EventBody::TryOn(t) => (
                        t.comfort.as_ref().map(|a| a.rating),
                        t.style.as_ref().map(|a| a.rating),
                        None,
                        None,
                        false,
                    ),
                    EventBody::Wear(w) => (
                        w.comfort.as_ref().map(|a| a.rating),
                        None,
                        None,
                        None,
                        false,
                    ),
                    EventBody::Feedback(f) => (
                        f.comfort.as_ref().map(|a| a.rating),
                        f.style.as_ref().map(|a| a.rating),
                        f.quality.as_ref().map(|a| a.rating),
                        f.value.as_ref().map(|a| a.rating),
                        f.regret.is_some(),
                    ),
                    _ => (None, None, None, None, false),
                };
                let ratings = [comfort, style, quality, value];
                let negatives: Vec<&str> = ["comfort", "style", "quality", "value"]
                    .into_iter()
                    .zip(ratings)
                    .filter(|(_, r)| adverse_rating(*r))
                    .map(|(name, _)| name)
                    .collect();
                let mixed = ratings.contains(&Some(Rating::Mixed));
                let overall = match body {
                    EventBody::TryOn(t) => Some(t.overall),
                    _ => None,
                };
                let slight: Vec<String> = body
                    .region_fit()
                    .iter()
                    .filter(|fit| fit::is_slight(fit.verdict))
                    .map(|fit| format!("{} {}", fit.region, enum_text(&fit.verdict)))
                    .collect();
                let has_signal =
                    !body.region_fit().is_empty() || ratings.iter().any(Option::is_some) || regret;
                if !has_signal {
                    None
                } else {
                    let mut detail = Vec::new();
                    if !adverse_regions.is_empty() {
                        detail.push(format!("adverse fit: {}", adverse_regions.join(", ")));
                    }
                    if !slight.is_empty() {
                        detail.push(format!("slight: {}", slight.join(", ")));
                    }
                    if !negatives.is_empty() {
                        detail.push(format!("negative: {}", negatives.join(", ")));
                    }
                    if regret {
                        detail.push("delayed regret".to_owned());
                    }
                    if let Some(overall) = overall {
                        detail.push(format!("overall {}", enum_text(&overall)));
                    }
                    let unknown = body
                        .region_fit()
                        .iter()
                        .any(|f| f.verdict == crate::model::FitVerdict::Unknown);
                    // A success needs positive fit: a known region verdict or
                    // a fitting try-on. Comfort, style or value alone, or only
                    // unknown regions, leave the fit unresolved.
                    let fit_reported = fit::known_fits(body.region_fit()).next().is_some()
                        || overall == Some(TryOnOverall::Fits);
                    let bucket = if !adverse_regions.is_empty()
                        || !negatives.is_empty()
                        || regret
                        || overall == Some(TryOnOverall::WrongSize)
                    {
                        Bucket::Failure
                    } else if fit_reported
                        && !mixed
                        && !unknown
                        && !matches!(
                            overall,
                            Some(TryOnOverall::Close | TryOnOverall::Unresolved)
                        )
                    {
                        Bucket::Success
                    } else {
                        Bucket::Unresolved
                    };
                    if !fit_reported {
                        let positives: Vec<String> = ["comfort", "style", "quality", "value"]
                            .into_iter()
                            .zip(ratings)
                            .filter(|(_, r)| *r == Some(Rating::Positive))
                            .map(|(name, _)| format!("{name} positive"))
                            .collect();
                        detail.push(if positives.is_empty() {
                            "no regional fit reported".to_owned()
                        } else {
                            format!("no regional fit reported ({})", positives.join(", "))
                        });
                    }
                    if detail.is_empty() {
                        detail.push(if bucket == Bucket::Success {
                            "fit good in every reported region".to_owned()
                        } else {
                            "mixed or unknown report".to_owned()
                        });
                    }
                    let kind = match body {
                        EventBody::Feedback(f) if f.regret.is_some() => "regret",
                        _ => event.event_type.as_str(),
                    };
                    Some((bucket, kind, detail.join("; ")))
                }
            }
            EventBody::Wash(w) => {
                let changes: Vec<String> = w
                    .observed_changes
                    .iter()
                    .filter(|c| c.change != ChangeKind::NoChange)
                    .map(|c| format!("{} {}", c.region, enum_text(&c.change)))
                    .collect();
                (!changes.is_empty()).then(|| {
                    (
                        Bucket::Failure,
                        "wash",
                        format!("changed after washing: {}", changes.join(", ")),
                    )
                })
            }
            EventBody::Returned(_) => {
                let reasons: Vec<String> = piece_id
                    .map(|id| {
                        state
                            .piece_events(id)
                            .filter_map(|e| match &e.envelope.event {
                                EventBody::Decision(d) => Some(d),
                                _ => None,
                            })
                            .last()
                            .map(|d| {
                                d.reasons
                                    .iter()
                                    .map(|r| {
                                        let region = r
                                            .region
                                            .as_ref()
                                            .map(|g| format!(" ({g})"))
                                            .unwrap_or_default();
                                        format!("{}{region}", enum_text(&r.aspect))
                                    })
                                    .collect()
                            })
                            .unwrap_or_default()
                    })
                    .unwrap_or_default();
                Some((
                    Bucket::Failure,
                    "return",
                    if reasons.is_empty() {
                        "returned; no reasons recorded".to_owned()
                    } else {
                        format!("returned: {}", reasons.join(", "))
                    },
                ))
            }
            _ => None,
        };
        if let Some((bucket, kind, detail)) = classified {
            let item = entry(
                state,
                kind,
                Some(event),
                piece_id,
                prediction_id,
                &event.occurred_on,
                detail,
            );
            buckets.push((bucket, item, event.ledger_revision));
        }
    }
    for record in state.current_records() {
        match &record.body {
            RecordBody::Piece(_) => {
                let id = record.id.as_str();
                let kept = state.piece_events(id).any(|e| {
                    matches!(&e.envelope.event, EventBody::Decision(d) if d.decision == crate::model::KeepDecision::Keep)
                });
                if kept
                    && state.piece_status(id) == PieceStatus::Owned
                    && !assessed_pieces.contains(id)
                    && relevant(state, focus, Some(id))
                {
                    buckets.push((
                        Bucket::Unresolved,
                        entry(
                            state,
                            "kept_unassessed",
                            None,
                            Some(id),
                            None,
                            &day_of(record.recorded_at),
                            "kept, but no try-on, wear or feedback assesses its fit".to_owned(),
                        ),
                        record.ledger_revision,
                    ));
                }
            }
            RecordBody::Prediction(_) => {
                let id = record.id.as_str();
                if !predictions_with_outcome.contains(id) && relevant(state, focus, Some(id)) {
                    buckets.push((
                        Bucket::Unresolved,
                        entry(
                            state,
                            "forecast_without_outcome",
                            None,
                            None,
                            Some(id),
                            &day_of(record.created_at),
                            "frozen forecast with no outcome recorded".to_owned(),
                        ),
                        record.ledger_revision,
                    ));
                }
            }
            _ => {}
        }
    }
    buckets.sort_by(|a, b| (b.1.occurred_on.as_str(), b.2).cmp(&(a.1.occurred_on.as_str(), a.2)));
    let mut out = HistoryOut {
        successes: Vec::new(),
        failures: Vec::new(),
        unresolved: Vec::new(),
        totals: HistoryTotals::default(),
    };
    for (bucket, item, _) in buckets {
        let (list, total) = match bucket {
            Bucket::Success => (&mut out.successes, &mut out.totals.successes),
            Bucket::Failure => (&mut out.failures, &mut out.totals.failures),
            Bucket::Unresolved => (&mut out.unresolved, &mut out.totals.unresolved),
        };
        *total += 1;
        if list.len() < limit {
            list.push(item);
        }
    }
    out
}

fn brand_history(state: &LedgerState, focus: &Focus) -> Vec<BrandHistory> {
    let mut rows: BTreeMap<(String, String), BrandHistory> = BTreeMap::new();
    for record in state.current_records() {
        if record.kind != RecordKind::Piece {
            continue;
        }
        let id = record.id.as_str();
        let Some((_, variant)) = state.piece_variant(id) else {
            continue;
        };
        let wanted = match (&focus.brand, &focus.category) {
            (Some(brand), _) => same(brand, &variant.brand),
            (None, Some(category)) => categories_overlap(category, &variant.category),
            (None, None) => true,
        };
        if !wanted {
            continue;
        }
        let key = (
            variant.brand.trim().to_lowercase(),
            variant.model.trim().to_lowercase(),
        );
        let row = rows.entry(key).or_insert_with(|| BrandHistory {
            brand: variant.brand.clone(),
            model: Some(variant.model.clone()),
            pieces: 0,
            owned: 0,
            returned: 0,
            retired: 0,
            sizes: Vec::new(),
            adverse_reports: 0,
            positive_reports: 0,
        });
        row.pieces += 1;
        match state.piece_status(id) {
            PieceStatus::Owned => row.owned += 1,
            PieceStatus::Returned => row.returned += 1,
            PieceStatus::Retired => row.retired += 1,
            PieceStatus::Undecided | PieceStatus::ReturnPending => {}
        }
        if !row.sizes.contains(&variant.size_label) {
            row.sizes.push(variant.size_label.clone());
        }
        for event in state.piece_events(id) {
            let fits = event.envelope.event.region_fit();
            if fit::known_fits(fits).next().is_none() {
                continue;
            }
            if fits.iter().any(|f| fit::is_adverse(f.verdict)) {
                row.adverse_reports += 1;
            } else {
                row.positive_reports += 1;
            }
        }
    }
    let mut rows: Vec<BrandHistory> = rows.into_values().collect();
    rows.sort_by(|a, b| b.pieces.cmp(&a.pieces).then_with(|| a.brand.cmp(&b.brand)));
    rows.truncate(30);
    rows
}

fn preferences(state: &LedgerState, focus: &Focus, today: Date) -> PreferencesOut {
    let mut current = Vec::new();
    let mut recent_changes = Vec::new();
    for record in state.current_records() {
        let RecordBody::Preference(p) = &record.body else {
            continue;
        };
        if p.category.as_str() != "any" && !focus.category(&p.category) {
            continue;
        }
        current.push(record_item(state, record));
        let recent = date_of(&p.valid_from).is_some_and(|d| days_between(d, today) <= STALE_DAYS);
        if record.revision > 1 && recent {
            let previous = state
                .history
                .get(record.id.as_str())
                .and_then(|rows| rows.iter().rev().nth(1))
                .and_then(|row| serde_json::from_str::<RecordBody>(&row.payload).ok());
            if let Some(RecordBody::Preference(old)) = previous {
                recent_changes.push(PreferenceChange {
                    id: record.id.as_str().to_owned(),
                    valid_from: p.valid_from.clone(),
                    statement: p.statement.clone(),
                    previous_valid_from: old.valid_from,
                    previous_statement: old.statement,
                });
            }
        }
    }
    PreferencesOut {
        current,
        recent_changes,
    }
}

fn rules(state: &LedgerState, focus: &Focus) -> RulesOut {
    let mut out = RulesOut {
        active: Vec::new(),
        proposed: Vec::new(),
        needs_review: Vec::new(),
    };
    for record in state.current_records() {
        let RecordBody::Rule(rule) = &record.body else {
            continue;
        };
        if rule.status == RuleStatus::Retired {
            continue;
        }
        // A rule is relevant when its scope covers a recorded variant in focus.
        let in_focus = focus.is_empty()
            || rule
                .scope
                .brand
                .as_ref()
                .zip(focus.brand.as_ref())
                .is_some_and(|(a, b)| same(a, b))
            || rule
                .scope
                .category
                .as_ref()
                .zip(focus.category.as_ref())
                .is_some_and(|(a, b)| categories_overlap(a, b))
            || state.current_records().any(|r| match &r.body {
                RecordBody::Variant(v) => focus.variant(v) && fit::variant_in_scope(&rule.scope, v),
                _ => false,
            });
        if !in_focus {
            continue;
        }
        let checked: BTreeSet<&str> = rule.regression.iter().map(|c| c.case_id.as_str()).collect();
        let unchecked: Vec<&Event> = fit::cases_in_scope(state, &rule.scope)
            .into_iter()
            .filter(|e| {
                e.ledger_revision > record.ledger_revision && !checked.contains(e.id.as_str())
            })
            .collect();
        let item = RuleOut {
            rule: record_item(state, record),
            unchecked_cases: unchecked.iter().map(|e| e.id.as_str().to_owned()).collect(),
            adverse_unchecked: unchecked
                .iter()
                .filter(|e| {
                    e.envelope
                        .event
                        .region_fit()
                        .iter()
                        .any(|f| fit::is_adverse(f.verdict))
                        || matches!(&e.envelope.event, EventBody::Wash(_))
                })
                .map(|e| e.id.as_str().to_owned())
                .collect(),
        };
        match rule.status {
            RuleStatus::Active => {
                if !item.unchecked_cases.is_empty() {
                    out.needs_review.push(record.id.as_str().to_owned());
                }
                out.active.push(item);
            }
            RuleStatus::Proposed => out.proposed.push(item),
            RuleStatus::Retired => {}
        }
    }
    out
}

/// Measurements grouped by subject, dimension, method and wash state.
type MeasurementGroups<'a> =
    BTreeMap<(String, String, String, String), Vec<(&'a Record, &'a MeasurementRecord)>>;

fn contradictions(state: &LedgerState) -> Vec<ContradictionOut> {
    let mut out = Vec::new();
    // Repeated measurements of the same thing that disagree.
    let mut groups: MeasurementGroups<'_> = BTreeMap::new();
    for (record, m) in measurements(state) {
        let wash = m
            .garment_state
            .as_ref()
            .map(|s| enum_text(&s.wash))
            .unwrap_or_default();
        let subject = m
            .subject
            .id
            .as_ref()
            .map_or_else(|| "body".to_owned(), |id| id.as_str().to_owned());
        groups
            .entry((
                subject,
                m.dimension.as_str().to_owned(),
                enum_text(&m.method),
                wash,
            ))
            .or_default()
            .push((record, m));
    }
    for ((subject, dimension, method, _), mut list) in groups {
        list.sort_by(|(_, a), (_, b)| a.observed_on.cmp(&b.observed_on));
        for pair in list.windows(2) {
            let ((ra, a), (rb, b)) = (pair[0], pair[1]);
            let (Some(da), Some(db)) = (date_of(&a.observed_on), date_of(&b.observed_on)) else {
                continue;
            };
            let body = a.subject.kind == SubjectKind::Body;
            if body && days_between(da, db) > AGREEMENT_WINDOW_DAYS {
                continue;
            }
            if !body
                && state.piece_events(&subject).any(|e| {
                    matches!(e.event_type, EventType::Wash | EventType::Alteration)
                        && e.occurred_on.as_str() > a.observed_on.as_str()
                        && e.occurred_on.as_str() <= b.observed_on.as_str()
                })
            {
                continue;
            }
            let delta = (units::to_mm(b.value, b.unit) - units::to_mm(a.value, a.unit)).abs();
            let threshold = units::disagreement_threshold_mm(a, b);
            if delta > threshold {
                out.push(ContradictionOut {
                    source: "computed".to_owned(),
                    kind: if body { "body_measurement" } else { "garment_measurement" }.to_owned(),
                    description: format!(
                        "{subject} {dimension} ({method}) measured {} mm on {} and {} mm on {}: {} mm apart, beyond {} mm",
                        units::to_mm(a.value, a.unit),
                        a.observed_on,
                        units::to_mm(b.value, b.unit),
                        b.observed_on,
                        units::round1(delta),
                        units::round1(threshold)
                    ),
                    ids: vec![ra.id.as_str().to_owned(), rb.id.as_str().to_owned()],
                });
            }
        }
    }
    // Fit reports about the same region of the same piece that disagree with
    // no wash or alteration between them.
    let mut reports: BTreeMap<(String, String), Vec<(&Event, crate::model::FitVerdict)>> =
        BTreeMap::new();
    for event in state.current_events() {
        let Some(piece) = event.envelope.event.piece_id() else {
            continue;
        };
        if matches!(event.envelope.event, EventBody::Outcome(_)) {
            continue;
        }
        for fit in event.envelope.event.region_fit() {
            reports
                .entry((piece.as_str().to_owned(), fit.region.as_str().to_owned()))
                .or_default()
                .push((event, fit.verdict));
        }
    }
    for ((piece, region), list) in reports {
        for pair in list.windows(2) {
            let ((a, va), (b, vb)) = (pair[0], pair[1]);
            if fit::match_verdict(va, vb) != RegionMatch::Miss {
                continue;
            }
            let changed = state.piece_events(&piece).any(|e| {
                matches!(e.event_type, EventType::Wash | EventType::Alteration)
                    && e.ledger_revision > a.ledger_revision
                    && e.ledger_revision < b.ledger_revision
            });
            if changed {
                continue;
            }
            out.push(ContradictionOut {
                source: "computed".to_owned(),
                kind: "fit_report".to_owned(),
                description: format!(
                    "{piece} {region}: {} on {} but {} on {}, with no wash or alteration between",
                    enum_text(&va),
                    a.occurred_on,
                    enum_text(&vb),
                    b.occurred_on
                ),
                ids: vec![a.id.as_str().to_owned(), b.id.as_str().to_owned()],
            });
        }
    }
    // Opposing current preferences for the same category, aspect and region.
    let prefs: Vec<(&Record, &crate::model::PreferenceRecord)> = state
        .current_records()
        .filter_map(|r| match &r.body {
            RecordBody::Preference(p) => Some((r, p)),
            _ => None,
        })
        .collect();
    for (i, (ra, a)) in prefs.iter().enumerate() {
        for (rb, b) in &prefs[i + 1..] {
            if a.category == b.category
                && a.aspect == b.aspect
                && a.region == b.region
                && a.strength.is_positive() != b.strength.is_positive()
            {
                out.push(ContradictionOut {
                    source: "computed".to_owned(),
                    kind: "preference".to_owned(),
                    description: format!(
                        "opposing {} preferences for {}: \"{}\" and \"{}\"",
                        enum_text(&a.aspect),
                        a.category,
                        a.statement,
                        b.statement
                    ),
                    ids: vec![ra.id.as_str().to_owned(), rb.id.as_str().to_owned()],
                });
            }
        }
    }
    // Open contradiction events not resolved by a current resolution.
    let resolved: BTreeSet<&str> = state
        .current_events()
        .filter_map(|e| match &e.envelope.event {
            EventBody::Contradiction(c) if c.status == ContradictionStatus::Resolved => {
                c.resolves_event_id.as_ref().map(|id| id.as_str())
            }
            _ => None,
        })
        .collect();
    // A resolution without resolvesEventId closes the computed contradictions
    // whose ids it lists.
    let resolved_sets: Vec<BTreeSet<&str>> = state
        .current_events()
        .filter_map(|e| match &e.envelope.event {
            EventBody::Contradiction(c)
                if c.status == ContradictionStatus::Resolved && c.resolves_event_id.is_none() =>
            {
                Some(c.ids.iter().map(|id| id.as_str()).collect())
            }
            _ => None,
        })
        .collect();
    out.retain(|computed| {
        !resolved_sets
            .iter()
            .any(|set| computed.ids.iter().all(|id| set.contains(id.as_str())))
    });
    for event in state.current_events() {
        if let EventBody::Contradiction(c) = &event.envelope.event
            && c.status == ContradictionStatus::Open
            && !resolved.contains(event.id.as_str())
        {
            let mut ids = vec![event.id.as_str().to_owned()];
            ids.extend(c.ids.iter().map(|id| id.as_str().to_owned()));
            out.push(ContradictionOut {
                source: "flagged".to_owned(),
                kind: "flagged".to_owned(),
                description: c.description.clone(),
                ids,
            });
        }
    }
    out
}

/// Body dimensions that matter for a category's top level.
fn key_body_dimensions(category: &Category) -> &'static [&'static str] {
    match category.top() {
        "tops" | "shirts" | "tshirts" | "knitwear" | "sweaters" | "outerwear" | "jackets"
        | "coats" | "suits" | "tailoring" => &["chest", "shoulder_width", "arm_length"],
        "bottoms" | "trousers" | "pants" | "jeans" | "shorts" => {
            &["waist_trouser", "hip", "inseam", "thigh"]
        }
        "footwear" | "shoes" | "boots" | "sneakers" => &["foot_length", "foot_width"],
        _ => &[],
    }
}

#[allow(clippy::too_many_arguments)]
fn warnings(
    state: &LedgerState,
    focus: &Focus,
    requested: &crate::api::Focus,
    profile: &ProfileOut,
    references: &[ReferenceOut],
    contradictions: &[ContradictionOut],
    rules: &RulesOut,
    today: Date,
) -> Vec<WarningOut> {
    let mut out = Vec::new();
    let mut warn = |code: &str, severity: &str, message: String, ids: Vec<String>| {
        out.push(WarningOut {
            code: code.to_owned(),
            severity: severity.to_owned(),
            message,
            ids,
        });
    };
    if profile.latest_snapshot.is_none() && profile.body_measurements.is_empty() {
        warn(
            "no_profile",
            "warning",
            "no profile snapshot or body measurements are recorded; onboarding has not happened"
                .to_owned(),
            Vec::new(),
        );
    }
    if let Some(snapshot) = &profile.latest_snapshot {
        let observed = snapshot
            .data
            .get("observedOn")
            .and_then(Value::as_str)
            .unwrap_or("");
        if date_of(observed).is_some_and(|d| days_between(d, today) > STALE_DAYS) {
            warn(
                "profile_stale",
                "info",
                format!("the newest profile snapshot is from {observed}"),
                vec![snapshot.id.clone()],
            );
        }
    }
    for body in &profile.body_measurements {
        if body.stale {
            warn(
                "body_measurement_stale",
                "info",
                format!(
                    "body {} ({}) was last measured on {}",
                    body.dimension, body.method, body.latest.observed_on
                ),
                vec![body.latest.id.clone()],
            );
        }
    }
    if let Some(category) = &focus.category {
        let dims = key_body_dimensions(category);
        let missing: Vec<&str> = dims
            .iter()
            .filter(|d| !profile.body_measurements.iter().any(|b| b.dimension == **d))
            .copied()
            .collect();
        if !missing.is_empty() {
            warn(
                "body_measurements_missing",
                "warning",
                format!(
                    "no body measurement for {} ({category})",
                    missing.join(", ")
                ),
                Vec::new(),
            );
        }
        let inferred: Vec<String> = profile
            .body_measurements
            .iter()
            .filter(|b| {
                dims.contains(&b.dimension.as_str())
                    && b.certainty != enum_text(&Certainty::Explicit)
            })
            .map(|b| b.dimension.clone())
            .collect();
        if !inferred.is_empty() {
            warn(
                "body_measurements_inferred",
                "info",
                format!(
                    "only inferred or unknown values for {}",
                    inferred.join(", ")
                ),
                Vec::new(),
            );
        }
        let baselines: Vec<&ReferenceOut> = references
            .iter()
            .filter(|r| r.role == enum_text(&ReferenceRole::Baseline))
            .collect();
        let current_baselines: Vec<&&ReferenceOut> = baselines
            .iter()
            .filter(|r| r.piece.status == PieceStatus::Owned.as_str())
            .collect();
        let category_fit_reports = state.current_events().any(|e| {
            fit::known_fits(e.envelope.event.region_fit())
                .next()
                .is_some()
                && e.envelope
                    .event
                    .piece_id()
                    .and_then(|id| state.piece(id.as_str()))
                    .is_some_and(|p| categories_overlap(category, &p.category))
        });
        if current_baselines.is_empty() {
            let blocking = missing.len() == dims.len() && !category_fit_reports;
            warn(
                if blocking {
                    "no_fit_evidence"
                } else {
                    "no_reference_garment"
                },
                if blocking { "blocking" } else { "warning" },
                if blocking {
                    format!(
                        "no owned baseline reference, body measurement or fit history for {category}; abstain from size advice until onboarding records one"
                    )
                } else {
                    format!("no owned baseline reference garment for {category}")
                },
                Vec::new(),
            );
        }
        for reference in &baselines {
            if reference.piece.status != PieceStatus::Owned.as_str() {
                warn(
                    "reference_not_current",
                    "warning",
                    format!(
                        "reference {} is {}",
                        reference.piece.id, reference.piece.status
                    ),
                    vec![reference.piece.id.clone()],
                );
            } else if reference.measurements.is_empty() {
                warn(
                    "reference_unmeasured",
                    "warning",
                    format!("reference {} has no measurements", reference.piece.id),
                    vec![reference.piece.id.clone()],
                );
            } else if !reference.washed_baseline {
                warn(
                    "reference_unwashed",
                    "warning",
                    format!(
                        "reference {} has measurements that are not from a washed state",
                        reference.piece.id
                    ),
                    vec![reference.piece.id.clone()],
                );
            }
            if reference.alterations > 0 {
                warn(
                    "reference_altered",
                    "info",
                    format!(
                        "reference {} was altered; use measurements taken after the alteration",
                        reference.piece.id
                    ),
                    vec![reference.piece.id.clone()],
                );
            }
        }
    }
    if let Some(variant_id) = &requested.variant_id {
        let published = measurements(state).any(|(_, m)| {
            m.subject.kind == SubjectKind::VariantPublished
                && m.subject.id.as_ref() == Some(variant_id)
        });
        if !published {
            warn(
                "candidate_unmeasured",
                "info",
                format!("no published measurements for {variant_id}; triangulate and say so"),
                vec![variant_id.as_str().to_owned()],
            );
        }
    }
    if !contradictions.is_empty() {
        warn(
            "open_contradictions",
            "warning",
            format!("{} unresolved contradictions", contradictions.len()),
            Vec::new(),
        );
    }
    if !rules.needs_review.is_empty() {
        warn(
            "rules_need_review",
            "warning",
            format!(
                "{} active rules have unchecked in-scope cases; test them before relying on them",
                rules.needs_review.len()
            ),
            rules.needs_review.clone(),
        );
    }
    if state.revision < state.current_revision {
        warn(
            "historical_snapshot",
            "info",
            format!(
                "this context is as of revision {}; the newest is {}",
                state.revision, state.current_revision
            ),
            Vec::new(),
        );
    }
    out
}

fn due(state: &LedgerState, today: Date) -> Vec<DueItem> {
    let mut out = Vec::new();
    let label = |id: &str| {
        state
            .piece(id)
            .map(|p| p.label.clone())
            .unwrap_or_else(|| id.to_owned())
    };
    let mut acquired_predictions: BTreeMap<String, String> = BTreeMap::new();
    for record in state.current_records() {
        if record.kind != RecordKind::Piece {
            continue;
        }
        let id = record.id.as_str();
        let acquisition = state.acquisition(id);
        if let Some((_, acquired)) = acquisition
            && let Some(prediction) = &acquired.prediction_id
        {
            acquired_predictions.insert(prediction.as_str().to_owned(), id.to_owned());
        }
        let status = state.piece_status(id);
        if status.is_open_decision() {
            let return_by = acquisition
                .and_then(|(_, a)| a.order.as_ref())
                .and_then(|o| o.return_by.clone());
            let days_left = return_by
                .as_deref()
                .and_then(date_of)
                .map(|d| days_between(today, d));
            out.push(DueItem {
                kind: "return_decision".to_owned(),
                id: id.to_owned(),
                label: label(id),
                due_on: return_by,
                days_left,
                detail: format!(
                    "{}: decide keep or return; do not wash, alter or wear it beyond a try-on",
                    status.as_str()
                ),
            });
        }
    }
    let outcomes: Vec<(&str, FeedbackTiming)> = state
        .current_events()
        .filter_map(|e| match &e.envelope.event {
            EventBody::Outcome(o) => Some((o.prediction_id.as_str(), o.stage)),
            _ => None,
        })
        .collect();
    for record in state.current_records() {
        if record.kind != RecordKind::Prediction {
            continue;
        }
        let id = record.id.as_str();
        let has_outcome = outcomes.iter().any(|(p, _)| *p == id);
        let created = date_of(&day_of(record.created_at)).unwrap_or(today);
        let age = days_between(created, today);
        match acquired_predictions.get(id) {
            Some(piece) if !has_outcome => out.push(DueItem {
                kind: "outcome_follow_up".to_owned(),
                id: id.to_owned(),
                label: label(piece),
                due_on: None,
                days_left: None,
                detail: format!("{piece} was acquired for this forecast; record the try-on outcome"),
            }),
            Some(piece) => {
                let washed = state
                    .piece_events(piece)
                    .any(|e| e.event_type == EventType::Wash);
                let after_wash = outcomes
                    .iter()
                    .any(|(p, stage)| *p == id && *stage == FeedbackTiming::AfterWash);
                if state.piece_status(piece) == PieceStatus::Owned && !after_wash {
                    out.push(DueItem {
                        kind: "post_wash_check".to_owned(),
                        id: id.to_owned(),
                        label: label(piece),
                        due_on: None,
                        days_left: None,
                        detail: if washed {
                            format!("{piece} has been washed; re-measure and record an after_wash outcome")
                        } else {
                            format!("after the first wash of {piece}, re-measure and record an after_wash outcome")
                        },
                    });
                }
            }
            None if !has_outcome && age >= FOLLOW_UP_DAYS => out.push(DueItem {
                kind: "outcome_follow_up".to_owned(),
                id: id.to_owned(),
                label: id.to_owned(),
                due_on: None,
                days_left: None,
                detail: format!(
                    "frozen {age} days ago with no acquisition or outcome; record not_purchased or the result"
                ),
            }),
            None => {}
        }
    }
    for record in state.current_records() {
        let RecordBody::Need(need) = &record.body else {
            continue;
        };
        if !matches!(need.status, NeedStatus::Open | NeedStatus::Paused) {
            continue;
        }
        let Some(review_after) = &need.review_after else {
            continue;
        };
        if let Some(date) = date_of(review_after)
            && date <= today
        {
            out.push(DueItem {
                kind: "need_review".to_owned(),
                id: record.id.as_str().to_owned(),
                label: need.description.chars().take(80).collect(),
                due_on: Some(review_after.clone()),
                days_left: Some(days_between(today, date)),
                detail: format!("{} need for {}", enum_text(&need.reason), need.category),
            });
        }
    }
    out.sort_by(|a, b| {
        a.days_left
            .unwrap_or(i64::MAX)
            .cmp(&b.days_left.unwrap_or(i64::MAX))
    });
    out
}

impl crate::service::Stylist {
    /// The context now, or as of an earlier revision. A historical context is
    /// evaluated on the date of that revision's last write, so the same
    /// revision always yields the same context and digest.
    pub async fn context(&self, input: ContextInput) -> Result<ContextOutput, StylistError> {
        if let Some(token) = &input.context_token {
            return self.rerender(&input, token).await;
        }
        let state = self.state(&input.namespace, input.as_of_revision).await?;
        let historical = state.revision < state.current_revision;
        let today = match state.latest_recorded_at() {
            Some(ms) if historical => self.date_of(ms),
            _ => self.today(),
        };
        build(&state, &input, today, self.clock.now_ms())
    }

    /// Exactly the context a token names, checked against its digest.
    async fn rerender(
        &self,
        input: &ContextInput,
        token: &ContextToken,
    ) -> Result<ContextOutput, StylistError> {
        if input.focus.is_some()
            || input.as_of_revision.is_some()
            || input.runbook_version.is_some()
            || input.limit.is_some()
        {
            return Err(crate::error::invalid(
                "contextToken already names the focus, revision, runbook version and limit; omit them",
            ));
        }
        let (params, digest) = parse_token(token)?;
        if params.ns != input.namespace.as_str() {
            return Err(mismatch(format!(
                "the context token is for namespace {}, not {}",
                params.ns, input.namespace
            )));
        }
        let state = self.state(&input.namespace, Some(params.rev)).await?;
        let mut output = render(&state, &params)?;
        if output.snapshot.context_sha256 != digest {
            return Err(mismatch(format!(
                "the context token does not match the ledger at revision {}",
                params.rev
            )));
        }
        output.snapshot.generated_at = iso(self.clock.now_ms());
        if input.include_runbook.unwrap_or(true) {
            output.runbook.text = runbook::find(&params.runbook).map(|r| r.text.to_owned());
        }
        Ok(output)
    }
}
