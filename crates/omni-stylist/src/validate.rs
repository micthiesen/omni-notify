//! Write validation against the loaded ledger, and the values the server
//! derives at write time. Everything here is pure: the service runs it inside
//! the write transaction, after the idempotency check and before any insert.

use jiff::civil::Date;
use serde_json::{Map, Value, json};

use crate::api::{AppendEventInput, SaveRecordInput};
use crate::context::ContextProof;
use crate::error::{StylistError, invalid, not_found, reject};
use crate::fit::{self, RegionMatch};
use crate::model::{
    EventBody, MeasurementRecord, PredictionDecision, PredictionRecord, Provenance, RecordBody,
    RecordKind, RegressionVerdict, RuleRecord, RuleStatus, SubjectKind, VariantRecord,
};
use crate::state::{EventEnvelope, LedgerState};
use crate::types::{EventId, EvidenceId, LedgerId, RecordId, new_id, parse_date};
use crate::units::{self, MAX_MM, SubjectClass, subject_class};

/// What a record write will store.
#[derive(Debug)]
pub struct RecordPlan {
    pub id: RecordId,
    pub kind: RecordKind,
    pub revision: u64,
    /// The body to store (the input, with unchanged creation-only fields kept).
    pub body: RecordBody,
    pub derived: Option<Map<String, Value>>,
    pub warnings: Vec<String>,
}

/// What an event append will store.
#[derive(Debug)]
pub struct EventPlan {
    pub id: EventId,
    pub envelope: EventEnvelope,
    pub derived: Option<Map<String, Value>>,
    pub warnings: Vec<String>,
}

fn date(field: &str, value: &str) -> Result<Date, StylistError> {
    parse_date(value).map_err(|e| invalid(format!("{field}: {e}")))
}

/// Dates of things that happened may not be in the future (one day of slack
/// for time zones).
fn past_date(field: &str, value: &str, today: Date) -> Result<Date, StylistError> {
    let parsed = date(field, value)?;
    if parsed > today.tomorrow().unwrap_or(today) {
        return Err(invalid(format!(
            "{field} {value} is in the future (today is {today})"
        )));
    }
    Ok(parsed)
}

fn evidence_exists(state: &LedgerState, ids: &[EvidenceId]) -> Result<(), StylistError> {
    for id in ids {
        if !state.evidence.contains_key(id.as_str()) {
            return Err(not_found(format!(
                "no evidence {id} in namespace {}",
                state.namespace
            )));
        }
    }
    Ok(())
}

fn provenance_ok(state: &LedgerState, p: &Provenance, today: Date) -> Result<(), StylistError> {
    if let Some(observed) = &p.observed_on {
        past_date("provenance.observedOn", observed, today)?;
    }
    evidence_exists(state, &p.evidence_ids)
}

fn ledger_id_exists(state: &LedgerState, id: &LedgerId) -> Result<(), StylistError> {
    if state.has_id(id.as_str()) {
        Ok(())
    } else {
        Err(not_found(format!(
            "no record, event or evidence {id} in namespace {}",
            state.namespace
        )))
    }
}

fn record_exists(
    state: &LedgerState,
    id: &RecordId,
    kinds: &[RecordKind],
) -> Result<(), StylistError> {
    match state.record(id.as_str()) {
        Some(record) if kinds.contains(&record.kind) => Ok(()),
        Some(record) => Err(invalid(format!(
            "{id} is a {}, expected {}",
            record.kind.as_str(),
            kinds
                .iter()
                .map(|k| k.as_str())
                .collect::<Vec<_>>()
                .join(" or ")
        ))),
        None => Err(not_found(format!(
            "no record {id} in namespace {}",
            state.namespace
        ))),
    }
}

fn norm(text: &str) -> String {
    text.split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

fn opt_norm(text: Option<&String>) -> String {
    text.map(|t| norm(t)).unwrap_or_default()
}

/// The identity of a variant for duplicate detection.
pub fn variant_key(v: &VariantRecord) -> String {
    [
        norm(&v.brand),
        norm(&v.model),
        norm(&v.size_label),
        opt_norm(v.cut.as_ref()),
        opt_norm(v.colour.as_ref()),
        opt_norm(v.version.as_ref()),
        opt_norm(v.sku.as_ref()),
    ]
    .join("|")
}

/// Plans a record write: revision checks, immutability, references and the
/// kind-specific rules.
/// `proof` is the verified context token of a prediction (None otherwise).
pub fn plan_record(
    state: &LedgerState,
    input: &SaveRecordInput,
    today: Date,
    proof: Option<&ContextProof>,
) -> Result<RecordPlan, StylistError> {
    let kind = input.record.kind();
    let mut warnings = Vec::new();
    let (id, revision) = match &input.id {
        Some(id) => {
            let existing = state.record(id.as_str()).ok_or_else(|| {
                not_found(format!("no record {id} in namespace {}", state.namespace))
            })?;
            if existing.kind != kind {
                return Err(invalid(format!(
                    "{id} is a {}; a record's kind cannot change",
                    existing.kind.as_str()
                )));
            }
            if kind.is_immutable() {
                return Err(reject(
                    "immutable",
                    format!(
                        "{} records are immutable; save a new record with supersedes: {id} and a correctionReason",
                        kind.as_str()
                    ),
                ));
            }
            let Some(expected) = input.expected_revision else {
                return Err(invalid("expectedRevision is required to revise a record"));
            };
            if expected != existing.revision {
                return Err(reject(
                    "revision_conflict",
                    format!(
                        "{id} is at revision {}, not {expected}; re-read it and apply your change to the current revision",
                        existing.revision
                    ),
                ));
            }
            if input.supersedes.is_some() {
                return Err(invalid(
                    "supersedes applies only when creating a correcting record",
                ));
            }
            (id.clone(), existing.revision + 1)
        }
        None => {
            if input.expected_revision.is_some() {
                return Err(invalid("expectedRevision is only valid with id"));
            }
            (RecordId(new_id(kind.prefix())), 1)
        }
    };
    match (&input.supersedes, &input.correction_reason) {
        (Some(old), Some(_)) => {
            if !kind.is_immutable() {
                return Err(invalid(format!(
                    "{} records are revised in place with id and expectedRevision, not superseded",
                    kind.as_str()
                )));
            }
            record_exists(state, old, &[kind])?;
            if let Some(newer) = state.record_superseded_by(old.as_str()) {
                return Err(reject(
                    "revision_conflict",
                    format!("{old} is already superseded by {newer}; correct {newer} instead"),
                ));
            }
            if kind == RecordKind::Prediction
                && let Some(event) = prediction_events(state, old.as_str()).next()
            {
                return Err(reject(
                    "post_hoc",
                    format!(
                        "prediction {old} already has event {}; a forecast cannot be replaced after anything about its outcome was recorded",
                        event.id
                    ),
                ));
            }
        }
        (Some(_), None) => return Err(invalid("supersedes requires a correctionReason")),
        (None, Some(_)) => return Err(invalid("correctionReason applies only with supersedes")),
        (None, None) => {}
    }
    let mut body = input.record.clone();
    let derived = match &input.record {
        RecordBody::Variant(v) => {
            provenance_ok(state, &v.provenance, today)?;
            let key = variant_key(v);
            if input.id.is_none() && !v.allow_duplicate.unwrap_or(false) {
                let duplicate = state.current_records().find(|r| match &r.body {
                    RecordBody::Variant(existing) => variant_key(existing) == key,
                    _ => false,
                });
                if let Some(duplicate) = duplicate {
                    return Err(reject(
                        "duplicate_variant",
                        format!(
                            "variant {} already records this brand, model, size, cut, colour, version and SKU; reuse it or set allowDuplicate",
                            duplicate.id
                        ),
                    ));
                }
            }
            if let Some(id) = &input.id
                && let Some(old) = state.variant(id.as_str())
                && variant_identity(old) != variant_identity(v)
                && let Some(user) = variant_users(state, id.as_str()).first()
            {
                return Err(reject(
                    "immutable",
                    format!(
                        "variant {id} is used by {user}; its brand, model, category, size, cut, colour, version and SKU cannot change. Save a new variant instead"
                    ),
                ));
            }
            if let Some(fabric) = &v.fabric {
                let total: f64 = fabric.composition.iter().map(|f| f.percent).sum();
                if !fabric.composition.is_empty() && (total - 100.0).abs() > 1.0 {
                    warnings.push(format!("fabric composition sums to {total}%, not 100%"));
                }
            }
            None
        }
        RecordBody::Piece(p) => {
            provenance_ok(state, &p.provenance, today)?;
            if let Some(variant_id) = &p.variant_id {
                record_exists(state, variant_id, &[RecordKind::Variant])?;
                if let Some(variant) = state.variant(variant_id.as_str())
                    && variant.category != p.category
                {
                    warnings.push(format!(
                        "piece category {} differs from its variant's {}",
                        p.category, variant.category
                    ));
                }
            } else if p.description.is_none() {
                warnings.push(
                    "no variantId or description: brand, model and size are unknown for this piece"
                        .to_owned(),
                );
            }
            if let Some(id) = &input.id
                && let Some(old) = state.piece(id.as_str())
            {
                if p.initial_status.is_some() && old.initial_status != p.initial_status {
                    return Err(invalid(
                        "initialStatus cannot change after creation; omit it on revisions and append a decision, returned or retired event",
                    ));
                }
                if old.variant_id != p.variant_id
                    && old.variant_id.is_some()
                    && let Some(user) = piece_users(state, id.as_str()).first()
                {
                    return Err(reject(
                        "immutable",
                        format!(
                            "piece {id} has history ({user}) recorded against its variant; save a new piece instead of re-pointing it"
                        ),
                    ));
                }
                if p.initial_status.is_none() && old.initial_status.is_some() {
                    let mut kept = p.clone();
                    kept.initial_status = old.initial_status;
                    body = RecordBody::Piece(kept);
                }
            }
            None
        }
        RecordBody::Measurement(m) => Some(measurement(state, m, today, input, &mut warnings)?),
        RecordBody::ProfileSnapshot(p) => {
            provenance_ok(state, &p.provenance, today)?;
            past_date("observedOn", &p.observed_on, today)?;
            let mut names = std::collections::HashSet::new();
            for attribute in &p.attributes {
                if !names.insert(attribute.name.as_str()) {
                    return Err(invalid(format!(
                        "attribute {} appears twice",
                        attribute.name
                    )));
                }
                if attribute.number.is_none() && attribute.text.is_none() {
                    return Err(invalid(format!(
                        "attribute {} needs a number or text",
                        attribute.name
                    )));
                }
            }
            Some(profile_derived(p))
        }
        RecordBody::Preference(p) => {
            provenance_ok(state, &p.provenance, today)?;
            let valid_from = date("validFrom", &p.valid_from)?;
            if let Some(RecordBody::Preference(old)) = input
                .id
                .as_ref()
                .and_then(|id| state.record(id.as_str()))
                .map(|r| &r.body)
                && valid_from < date("validFrom", &old.valid_from)?
            {
                return Err(invalid(format!(
                    "validFrom {} is earlier than the current revision's {}; record an older period as its own preference",
                    p.valid_from, old.valid_from
                )));
            }
            None
        }
        RecordBody::Prediction(p) => Some(prediction(state, p, proof, &mut warnings)?),
        RecordBody::Rule(r) => Some(rule(state, r, &mut warnings)?),
        RecordBody::Need(n) => {
            provenance_ok(state, &n.provenance, today)?;
            for id in &n.related_piece_ids {
                record_exists(state, id, &[RecordKind::Piece])?;
            }
            if let Some(review_after) = &n.review_after {
                date("reviewAfter", review_after)?;
            }
            None
        }
    };
    Ok(RecordPlan {
        id,
        kind,
        revision,
        body,
        derived,
        warnings,
    })
}

/// The fields that make a variant the product it is.
fn variant_identity(v: &VariantRecord) -> (String, String, crate::model::SizeSystem) {
    (
        variant_key(v),
        v.category.as_str().to_owned(),
        v.size_system,
    )
}

/// Pieces, predictions and published measurements that use variant `id`.
fn variant_users(state: &LedgerState, id: &str) -> Vec<String> {
    state
        .records
        .values()
        .filter(|r| match &r.body {
            RecordBody::Piece(p) => p.variant_id.as_ref().is_some_and(|v| v.as_str() == id),
            RecordBody::Prediction(p) => p.variant_id.as_str() == id,
            RecordBody::Measurement(m) => m.subject.id.as_ref().is_some_and(|s| s.as_str() == id),
            _ => false,
        })
        .map(|r| r.id.as_str().to_owned())
        .collect()
}

/// Events (superseded ones included) and measurements recorded for piece `id`.
fn piece_users(state: &LedgerState, id: &str) -> Vec<String> {
    let events = state
        .events
        .iter()
        .filter(|e| {
            e.envelope
                .event
                .piece_id()
                .is_some_and(|p| p.as_str() == id)
        })
        .map(|e| e.id.as_str().to_owned());
    let measurements = state
        .records
        .values()
        .filter(|r| match &r.body {
            RecordBody::Measurement(m) => m.subject.id.as_ref().is_some_and(|s| s.as_str() == id),
            _ => false,
        })
        .map(|r| r.id.as_str().to_owned());
    events.chain(measurements).collect()
}

/// Every event (superseded ones included) that references prediction `id`.
fn prediction_events<'a>(
    state: &'a LedgerState,
    id: &'a str,
) -> impl Iterator<Item = &'a crate::state::Event> + 'a {
    state.events.iter().filter(move |e| {
        e.envelope
            .event
            .prediction_id()
            .is_some_and(|p| p.as_str() == id)
    })
}

/// A frozen forecast may only be linked to a piece of its own variant whose
/// fit was not observed before the freeze, by events dated on or after it.
fn check_forecast_link(
    state: &LedgerState,
    prediction_id: &RecordId,
    piece_id: Option<&RecordId>,
    occurred_on: &str,
) -> Result<(), StylistError> {
    let Some(frozen) = state.record(prediction_id.as_str()) else {
        return Err(not_found(format!("no prediction {prediction_id}")));
    };
    if let Some(newer) = state.record_superseded_by(prediction_id.as_str()) {
        return Err(invalid(format!(
            "prediction {prediction_id} is superseded by {newer}; link the current forecast"
        )));
    }
    let RecordBody::Prediction(prediction) = &frozen.body else {
        return Err(invalid(format!("{prediction_id} is not a prediction")));
    };
    let frozen_on: String = crate::service::iso(frozen.created_at)
        .chars()
        .take(10)
        .collect();
    if let (Ok(event_day), Ok(freeze_day)) = (parse_date(occurred_on), parse_date(&frozen_on))
        && event_day < freeze_day.yesterday().unwrap_or(freeze_day)
    {
        return Err(reject(
            "post_hoc",
            format!(
                "this event is dated {occurred_on}, before prediction {prediction_id} was frozen on {frozen_on}"
            ),
        ));
    }
    let Some(piece_id) = piece_id else {
        return Ok(());
    };
    match state
        .piece(piece_id.as_str())
        .and_then(|p| p.variant_id.as_ref())
    {
        Some(variant) if *variant == prediction.variant_id => {}
        _ => {
            return Err(invalid(format!(
                "piece {piece_id} is not of variant {}, the forecast's exact variant",
                prediction.variant_id
            )));
        }
    }
    let observed = state.events.iter().find(|e| {
        e.ledger_revision < frozen.created_revision
            && e.envelope.event.piece_id() == Some(piece_id)
            && (e.envelope.event.is_fit_case()
                || matches!(
                    e.envelope.event,
                    EventBody::Decision(_) | EventBody::Returned(_)
                ))
    });
    if let Some(observed) = observed {
        return Err(reject(
            "post_hoc",
            format!(
                "piece {piece_id}'s fit was recorded ({}) before prediction {prediction_id} was frozen; a forecast cannot be scored against an outcome it could see",
                observed.id
            ),
        ));
    }
    Ok(())
}

fn measurement(
    state: &LedgerState,
    m: &MeasurementRecord,
    today: Date,
    input: &SaveRecordInput,
    warnings: &mut Vec<String>,
) -> Result<Map<String, Value>, StylistError> {
    provenance_ok(state, &m.provenance, today)?;
    past_date("observedOn", &m.observed_on, today)?;
    let class = subject_class(m.subject.kind);
    match (m.subject.kind, &m.subject.id) {
        (SubjectKind::Body, None) => {}
        (SubjectKind::Body, Some(_)) => {
            return Err(invalid("a body measurement has no subject id"));
        }
        (SubjectKind::Piece, Some(id)) => record_exists(state, id, &[RecordKind::Piece])?,
        (SubjectKind::VariantPublished, Some(id)) => {
            record_exists(state, id, &[RecordKind::Variant])?;
        }
        (_, None) => return Err(invalid("a garment measurement needs subject.id")),
    }
    if !units::method_allowed(class, m.method) {
        return Err(reject(
            "incompatible_measurement",
            "the body is measured as a circumference, linear length or curve, never as a flat width",
        ));
    }
    match class {
        SubjectClass::Body => {
            if m.garment_state.is_some() {
                return Err(invalid("garmentState applies only to garment measurements"));
            }
        }
        SubjectClass::Garment => {
            if m.body_state.is_some() {
                return Err(invalid("bodyState applies only to body measurements"));
            }
            let Some(state_of) = &m.garment_state else {
                return Err(invalid(
                    "garment measurements need garmentState (stretch and wash state)",
                ));
            };
            if let Some(after) = &state_of.after_event_id {
                match state.event(after.as_str()).map(|e| &e.envelope.event) {
                    Some(EventBody::Wash(wash)) => {
                        if m.subject.id.as_ref() != Some(&wash.piece_id) {
                            return Err(invalid(format!(
                                "afterEventId {after} is a wash of another piece"
                            )));
                        }
                    }
                    Some(_) => return Err(invalid(format!("afterEventId {after} is not a wash"))),
                    None => {
                        return Err(not_found(format!(
                            "no event {after} in namespace {}",
                            state.namespace
                        )));
                    }
                }
            }
        }
    }
    let value_mm = units::to_mm(m.value, m.unit);
    if value_mm <= 0.0 || value_mm > MAX_MM {
        return Err(invalid(format!(
            "{value_mm} mm is outside the plausible range (0, {MAX_MM}] mm"
        )));
    }
    if let Some(old) = &input.supersedes
        && let Some(RecordBody::Measurement(previous)) = state.record(old.as_str()).map(|r| &r.body)
        && (previous.dimension != m.dimension || previous.subject != m.subject)
    {
        warnings.push(format!(
            "this correction changes the subject or dimension of {old}"
        ));
    }
    let mut derived = Map::new();
    derived.insert("valueMm".into(), json!(value_mm));
    derived.insert(
        "toleranceMm".into(),
        m.tolerance
            .map_or(Value::Null, |t| json!(units::to_mm(t, m.unit))),
    );
    derived.insert(
        "subjectClass".into(),
        json!(match class {
            SubjectClass::Body => "body",
            SubjectClass::Garment => "garment",
        }),
    );
    Ok(derived)
}

fn profile_derived(p: &crate::model::ProfileSnapshotRecord) -> Map<String, Value> {
    use crate::model::ProfileUnit;
    let normalized: Vec<Value> = p
        .attributes
        .iter()
        .filter_map(|a| {
            let number = a.number?;
            let (value, unit) = match a.unit? {
                ProfileUnit::Mm => (number, "mm"),
                ProfileUnit::Cm => (number * 10.0, "mm"),
                ProfileUnit::In => (number * 25.4, "mm"),
                ProfileUnit::Kg => (number, "kg"),
                ProfileUnit::Lb => (number * 0.453_592_37, "kg"),
            };
            Some(json!({"name": a.name, "value": units::round1(value), "unit": unit}))
        })
        .collect();
    let mut derived = Map::new();
    derived.insert("normalized".into(), Value::Array(normalized));
    derived
}

fn prediction(
    state: &LedgerState,
    p: &PredictionRecord,
    proof: Option<&ContextProof>,
    warnings: &mut Vec<String>,
) -> Result<Map<String, Value>, StylistError> {
    let Some(proof) = proof else {
        return Err(invalid(
            "contextToken is required: pass snapshot.contextToken from the stylist_context_get result this forecast was made from",
        ));
    };
    if proof.params.rev != p.context_revision {
        return Err(reject(
            "context_mismatch",
            format!(
                "contextRevision {} differs from the token's revision {}",
                p.context_revision, proof.params.rev
            ),
        ));
    }
    record_exists(state, &p.variant_id, &[RecordKind::Variant])?;
    let Some(variant) = state.variant(p.variant_id.as_str()) else {
        return Err(not_found(format!("no variant {}", p.variant_id)));
    };
    if p.recommended_size.trim() != variant.size_label.trim() {
        return Err(invalid(format!(
            "recommendedSize {:?} differs from the variant's sizeLabel {:?}; save the variant for the recommended size",
            p.recommended_size, variant.size_label
        )));
    }
    if p.decision != PredictionDecision::Abstain && p.region_expectations.is_empty() {
        return Err(invalid(
            "a recommendation needs at least one region expectation (or decision: abstain)",
        ));
    }
    if p.context_revision > state.current_revision {
        return Err(invalid(format!(
            "contextRevision {} is after the namespace's newest revision {}",
            p.context_revision, state.current_revision
        )));
    }
    for id in &p.references_used {
        record_exists(
            state,
            id,
            &[
                RecordKind::Piece,
                RecordKind::Measurement,
                RecordKind::Variant,
            ],
        )?;
    }
    for id in &p.rules_applied {
        record_exists(state, id, &[RecordKind::Rule])?;
        if let Some(RecordBody::Rule(rule)) = state.record(id.as_str()).map(|r| &r.body) {
            if rule.status != RuleStatus::Active {
                warnings.push(format!("rule {id} is not active"));
            }
            if !fit::variant_in_scope(&rule.scope, variant) {
                warnings.push(format!("rule {id} does not cover this variant's scope"));
            }
        }
    }
    if let Some(need) = &p.need_id {
        record_exists(state, need, &[RecordKind::Need])?;
    }
    for expectation in &p.region_expectations {
        for id in &expectation.evidence_refs {
            ledger_id_exists(state, id)?;
        }
    }
    if let Some(check) = &p.shopping_check {
        date("shoppingCheck.checkedOn", &check.checked_on)?;
    }
    if let Some(return_by) = &p.return_risk.return_by {
        date("returnRisk.returnBy", return_by)?;
    }
    let assessment = fit::assess_basis(state, variant);
    let computed = assessment.basis;
    let why = if assessment.reasons.is_empty() {
        String::new()
    } else {
        format!(": {}", assessment.reasons.join("; "))
    };
    if p.basis > computed {
        return Err(reject(
            "basis_mismatch",
            format!(
                "basis {} is not supported; the ledger supports {}{why}",
                crate::state::enum_text(&p.basis),
                crate::state::enum_text(&computed),
            ),
        ));
    }
    if p.basis < computed {
        warnings.push(format!(
            "the ledger supports basis {}; {} was claimed",
            crate::state::enum_text(&computed),
            crate::state::enum_text(&p.basis)
        ));
    }
    if !assessment.known_failures.is_empty() {
        let failed = assessment.known_failures.join(", ");
        if p.decision == PredictionDecision::Recommend {
            return Err(reject(
                "known_failure",
                format!(
                    "this exact variant failed before ({failed}); use recommend_with_caution with the reason it should fit now, or abstain"
                ),
            ));
        }
        warnings.push(format!(
            "this exact variant failed before ({failed}); explain why the forecast differs"
        ));
    }
    let mixed: Vec<&str> = assessment
        .identity_matches
        .iter()
        .filter(|m| m.fit == fit::FitEvidence::Mixed)
        .map(|m| m.piece_id.as_str())
        .collect();
    if !mixed.is_empty() {
        warnings.push(format!(
            "the same model and size has mixed fit evidence ({})",
            mixed.join(", ")
        ));
    }
    let writes_since = state.current_revision.saturating_sub(p.context_revision);
    if writes_since > 0 {
        warnings.push(format!(
            "{writes_since} ledger writes happened after context revision {}; re-read the context if they matter",
            p.context_revision
        ));
    }
    let mut derived = Map::new();
    // What the forecast was made against, so later revisions cannot reinterpret it.
    let revision_of = |id: &str| state.record(id).map_or(0, |r| r.revision);
    derived.insert(
        "variantRevision".into(),
        json!(revision_of(p.variant_id.as_str())),
    );
    derived.insert(
        "variantSnapshot".into(),
        json!({
            "brand": variant.brand, "model": variant.model, "category": variant.category,
            "sizeLabel": variant.size_label, "sizeSystem": variant.size_system, "cut": variant.cut,
            "colour": variant.colour, "version": variant.version, "sku": variant.sku,
        }),
    );
    let revisions = |ids: &[RecordId]| -> Map<String, Value> {
        ids.iter()
            .map(|id| (id.as_str().to_owned(), json!(revision_of(id.as_str()))))
            .collect()
    };
    derived.insert(
        "referenceRevisions".into(),
        Value::Object(revisions(&p.references_used)),
    );
    derived.insert(
        "ruleRevisions".into(),
        Value::Object(revisions(&p.rules_applied)),
    );
    derived.insert("computedBasis".into(), json!(computed));
    derived.insert("basisRule".into(), json!("fit-evidence-v2"));
    derived.insert("basisAssessment".into(), json!(assessment));
    derived.insert("contextRevision".into(), json!(p.context_revision));
    derived.insert(
        "context".into(),
        json!({
            "verified": true,
            "revision": proof.params.rev,
            "today": proof.params.today,
            "focus": proof.params.focus,
            "limit": proof.params.limit,
            "runbookVersion": proof.params.runbook,
            "sha256": proof.sha256,
        }),
    );
    derived.insert("writesSinceContext".into(), json!(writes_since));
    Ok(derived)
}

fn rule(
    state: &LedgerState,
    r: &RuleRecord,
    warnings: &mut Vec<String>,
) -> Result<Map<String, Value>, StylistError> {
    if r.scope.brand.is_none() && r.scope.model.is_none() && r.scope.category.is_none() {
        return Err(invalid(
            "a rule's scope needs at least a brand, model or category; broad rules are not allowed",
        ));
    }
    for id in &r.evidence {
        ledger_id_exists(state, id)?;
    }
    for case in &r.regression {
        if state.event(case.case_id.as_str()).is_none() {
            return Err(not_found(format!(
                "regression case {} is not an event in namespace {}",
                case.case_id, state.namespace
            )));
        }
    }
    if let Some(old) = &r.supersedes_rule_id {
        record_exists(state, old, &[RecordKind::Rule])?;
    }
    let in_scope: Vec<String> = fit::cases_in_scope(state, &r.scope)
        .into_iter()
        .map(|e| e.id.as_str().to_owned())
        .collect();
    let checked: std::collections::HashSet<&str> =
        r.regression.iter().map(|c| c.case_id.as_str()).collect();
    let unchecked: Vec<String> = in_scope
        .iter()
        .filter(|id| !checked.contains(id.as_str()))
        .cloned()
        .collect();
    let inconsistent: Vec<&str> = r
        .regression
        .iter()
        .filter(|c| c.verdict == RegressionVerdict::Inconsistent)
        .map(|c| c.case_id.as_str())
        .collect();
    if r.status == RuleStatus::Active {
        if !unchecked.is_empty() {
            return Err(reject(
                "regression_incomplete",
                format!(
                    "activating this rule needs a regression entry for every in-scope fit case; unchecked: {}",
                    unchecked.join(", ")
                ),
            ));
        }
        if !inconsistent.is_empty() {
            return Err(reject(
                "regression_incomplete",
                format!(
                    "the rule is inconsistent with {}; narrow its scope or keep it proposed",
                    inconsistent.join(", ")
                ),
            ));
        }
    } else if !unchecked.is_empty() {
        warnings.push(format!(
            "{} in-scope fit cases are not yet in the regression list",
            unchecked.len()
        ));
    }
    let mut derived = Map::new();
    derived.insert("inScopeCases".into(), json!(in_scope));
    derived.insert("uncheckedCases".into(), json!(unchecked));
    Ok(derived)
}

/// Plans an event append.
pub fn plan_event(
    state: &LedgerState,
    input: &AppendEventInput,
    today: Date,
) -> Result<EventPlan, StylistError> {
    past_date("occurredOn", &input.occurred_on, today)?;
    evidence_exists(state, &input.evidence_ids)?;
    if let Some(provenance) = &input.provenance {
        provenance_ok(state, provenance, today)?;
    }
    let event_type = input.event.event_type();
    match (&input.supersedes_event_id, &input.correction_reason) {
        (Some(old), Some(_)) => {
            let Some(previous) = state.event(old.as_str()) else {
                return Err(not_found(format!(
                    "no event {old} in namespace {}",
                    state.namespace
                )));
            };
            if previous.event_type != event_type {
                return Err(invalid(format!(
                    "{old} is a {} event; a correction must have the same type",
                    previous.event_type.as_str()
                )));
            }
            if let Some(newer) = state.event_superseded_by(old.as_str()) {
                return Err(reject(
                    "revision_conflict",
                    format!("{old} is already superseded by {newer}; correct {newer} instead"),
                ));
            }
        }
        (Some(_), None) => return Err(invalid("supersedesEventId requires a correctionReason")),
        (None, Some(_)) => {
            return Err(invalid(
                "correctionReason applies only with supersedesEventId",
            ));
        }
        (None, None) => {}
    }
    let superseding = input.supersedes_event_id.as_ref().map(|id| id.as_str());
    let mut warnings = Vec::new();
    let mut derived = Map::new();
    let piece = |id: &RecordId| record_exists(state, id, &[RecordKind::Piece]);
    let prediction = |id: &RecordId| record_exists(state, id, &[RecordKind::Prediction]);
    let open_decision = |id: &RecordId| state.piece_status(id.as_str()).is_open_decision();
    match &input.event {
        EventBody::Acquired(e) => {
            piece(&e.piece_id)?;
            if let Some(id) = &e.prediction_id {
                prediction(id)?;
                check_forecast_link(state, id, Some(&e.piece_id), &input.occurred_on)?;
            }
            let earlier = state.piece_events(e.piece_id.as_str()).find(|ev| {
                ev.event_type == crate::model::EventType::Acquired
                    && Some(ev.id.as_str()) != superseding
            });
            if let Some(earlier) = earlier {
                return Err(reject(
                    "duplicate_receipt",
                    format!(
                        "piece {} already has acquisition {}; supersede it to correct it",
                        e.piece_id, earlier.id
                    ),
                ));
            }
            if let Some(order) = &e.order {
                if let Some(return_by) = &order.return_by {
                    date("order.returnBy", return_by)?;
                }
                if let Some(order_ref) = &order.order_ref {
                    let line = order
                        .line_ref
                        .clone()
                        .or_else(|| {
                            state
                                .piece(e.piece_id.as_str())
                                .and_then(|p| p.variant_id.as_ref())
                                .map(|v| v.as_str().to_owned())
                        })
                        .unwrap_or_else(|| e.piece_id.as_str().to_owned());
                    let key = format!(
                        "{}|{}|{}",
                        norm(&order.retailer),
                        norm(order_ref),
                        norm(&line)
                    );
                    let duplicate = state.current_events().find(|ev| {
                        Some(ev.id.as_str()) != superseding
                            && ev
                                .derived
                                .as_ref()
                                .and_then(|d| d.get("receiptKey"))
                                .and_then(Value::as_str)
                                == Some(key.as_str())
                    });
                    if let Some(duplicate) = duplicate {
                        return Err(reject(
                            "duplicate_receipt",
                            format!(
                                "order {order_ref} line {line} at {} is already recorded by {}",
                                order.retailer, duplicate.id
                            ),
                        ));
                    }
                    derived.insert("receiptKey".into(), json!(key));
                }
            }
        }
        EventBody::TryOn(e) => {
            if e.piece_id.is_none() && e.prediction_id.is_none() {
                return Err(invalid("a try-on needs a pieceId, a predictionId, or both"));
            }
            if let Some(id) = &e.piece_id {
                piece(id)?;
            }
            if let Some(id) = &e.prediction_id {
                prediction(id)?;
                check_forecast_link(state, id, e.piece_id.as_ref(), &input.occurred_on)?;
                insert_comparison(state, id, &e.region_fit, &mut derived);
            }
        }
        EventBody::Wear(e) => {
            piece(&e.piece_id)?;
            if open_decision(&e.piece_id) {
                warnings.push(format!(
                    "piece {} is undecided; wearing it beyond a try-on can void a return",
                    e.piece_id
                ));
            }
        }
        EventBody::Wash(e) => {
            piece(&e.piece_id)?;
            for id in &e.measurement_ids {
                record_exists(state, id, &[RecordKind::Measurement])?;
            }
            if open_decision(&e.piece_id) {
                warnings.push(format!(
                    "piece {} is undecided; washing it can void a return",
                    e.piece_id
                ));
            }
        }
        EventBody::Alteration(e) => {
            piece(&e.piece_id)?;
            if open_decision(&e.piece_id) {
                warnings.push(format!(
                    "piece {} is undecided; altering it voids most returns",
                    e.piece_id
                ));
            }
        }
        EventBody::Decision(e) => piece(&e.piece_id)?,
        EventBody::Returned(e) => {
            piece(&e.piece_id)?;
            if !open_decision(&e.piece_id) {
                warnings.push(format!(
                    "piece {} was {} before this return",
                    e.piece_id,
                    state.piece_status(e.piece_id.as_str()).as_str()
                ));
            }
        }
        EventBody::Retired(e) => piece(&e.piece_id)?,
        EventBody::Feedback(e) => {
            record_exists(
                state,
                &e.target_id,
                &[RecordKind::Piece, RecordKind::Prediction],
            )?;
            if e.target_id.as_str().starts_with("pred_") {
                check_forecast_link(state, &e.target_id, None, &input.occurred_on)?;
                insert_comparison(state, &e.target_id, &e.region_fit, &mut derived);
            }
        }
        EventBody::Outcome(e) => {
            prediction(&e.prediction_id)?;
            if let Some(id) = &e.piece_id {
                piece(id)?;
            }
            check_forecast_link(
                state,
                &e.prediction_id,
                e.piece_id.as_ref(),
                &input.occurred_on,
            )?;
            for id in &e.based_on_event_ids {
                if state.event(id.as_str()).is_none() {
                    return Err(not_found(format!(
                        "no event {id} in namespace {}",
                        state.namespace
                    )));
                }
            }
            let repeated = state.current_events().find(|ev| {
                Some(ev.id.as_str()) != superseding
                    && matches!(&ev.envelope.event, EventBody::Outcome(o)
                        if o.prediction_id == e.prediction_id && o.stage == e.stage)
            });
            if let Some(repeated) = repeated {
                return Err(reject(
                    "duplicate_outcome",
                    format!(
                        "prediction {} already has a {} outcome ({}); supersede it to correct it",
                        e.prediction_id,
                        crate::state::enum_text(&e.stage),
                        repeated.id
                    ),
                ));
            }
            insert_comparison(state, &e.prediction_id, &e.region_results, &mut derived);
        }
        EventBody::Contradiction(e) => {
            for id in &e.ids {
                ledger_id_exists(state, id)?;
            }
            match (e.status, &e.resolves_event_id) {
                (crate::model::ContradictionStatus::Resolved, Some(id)) => {
                    match state.event(id.as_str()).map(|ev| &ev.envelope.event) {
                        Some(EventBody::Contradiction(open))
                            if open.status == crate::model::ContradictionStatus::Open => {}
                        _ => {
                            return Err(invalid(format!(
                                "resolvesEventId {id} is not an open contradiction"
                            )));
                        }
                    }
                }
                (crate::model::ContradictionStatus::Resolved, None) => {
                    // Resolves a computed contradiction: list the ids it named.
                    if e.ids.len() < 2 {
                        return Err(invalid(
                            "a resolution needs resolvesEventId (a flagged contradiction) or the two or more ids of a computed one",
                        ));
                    }
                }
                (crate::model::ContradictionStatus::Open, Some(_)) => {
                    return Err(invalid("resolvesEventId applies only to status resolved"));
                }
                (crate::model::ContradictionStatus::Open, None) => {}
            }
        }
        EventBody::Note(e) => {
            for id in &e.ids {
                ledger_id_exists(state, id)?;
            }
        }
    }
    let envelope = EventEnvelope {
        occurred_on: input.occurred_on.clone(),
        event: input.event.clone(),
        evidence_ids: input.evidence_ids.clone(),
        provenance: Some(input.provenance.clone().unwrap_or(Provenance {
            certainty: crate::model::Certainty::Explicit,
            source: crate::model::Source::UserStatement,
            observed_on: None,
            evidence_ids: Vec::new(),
            note: None,
        })),
        notes: input.notes.clone(),
        supersedes_event_id: input.supersedes_event_id.clone(),
        correction_reason: input.correction_reason.clone(),
    };
    Ok(EventPlan {
        id: EventId(new_id("ev")),
        envelope,
        derived: (!derived.is_empty()).then_some(derived),
        warnings,
    })
}

/// Adds the comparison of reported fit with the prediction's frozen expectations.
fn insert_comparison(
    state: &LedgerState,
    prediction_id: &RecordId,
    reported: &[crate::model::RegionFit],
    derived: &mut Map<String, Value>,
) {
    let Some(record) = state.record(prediction_id.as_str()) else {
        return;
    };
    let RecordBody::Prediction(prediction) = &record.body else {
        return;
    };
    let comparison = fit::compare_regions(&prediction.region_expectations, reported);
    let misses = comparison
        .iter()
        .filter(|c| c.matched == RegionMatch::Miss)
        .count();
    derived.insert("forecastComparison".into(), json!(comparison));
    derived.insert("forecastMisses".into(), json!(misses));
    derived.insert("forecastRevision".into(), json!(record.revision));
    derived.insert(
        "forecastFrozenAtRevision".into(),
        json!(record.created_revision),
    );
}
