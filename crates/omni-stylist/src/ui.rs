//! The read-only web overview behind `GET /api/stylist` (omni-web's
//! `/stylist` page). It is assembled from the same context the MCP tools
//! serve. Evidence bytes, photos and notes beyond short labels are never
//! included: the LAN UI has no login, so private bytes stay behind MCP.

use std::collections::BTreeMap;

use axum::Router;
use axum::extract::{Query, State};
use axum::response::Response;
use axum::routing::get;
use omni_api::stylist::{
    StylistArchive, StylistAttribute, StylistBodyMeasurement, StylistCase, StylistContradiction,
    StylistCounts, StylistDue, StylistHistory, StylistMeasurement, StylistNamespace, StylistNeed,
    StylistOutcome, StylistOverview, StylistPiece, StylistPrediction, StylistPreference,
    StylistProfile, StylistRegion, StylistRule, StylistRunbook, StylistScore, StylistWarning,
};
use omni_server_kit::{ApiError, json_response};
use serde::Deserialize;
use serde_json::Value;

use crate::api::{ArchiveManifest, ContextInput, ContextOutput, HistoryEntry, MeasurementRef};
use crate::error::StylistError;
use crate::fit;
use crate::model::{
    EventBody, Fabric, MeasurementRecord, NeedStatus, PredictionRecord, Priority,
    ProfileSnapshotRecord, RecordBody, RecordKind,
};
use crate::service::{Stylist, iso, measurement_ref};
use crate::state::{LedgerState, Record, enum_text};
use crate::types::Namespace;
use crate::{context, ledger};

fn count(n: usize) -> f64 {
    u32::try_from(n).map_or(f64::MAX, f64::from)
}

fn shown(m: &MeasurementRef) -> StylistMeasurement {
    StylistMeasurement {
        id: m.id.clone(),
        dimension: m.dimension.clone(),
        method: m.method.clone(),
        value_mm: m.value_mm,
        original: format!("{} {}", omni_core::js::number_to_string(m.value), m.unit),
        observed_on: m.observed_on.clone(),
        endpoints: m.endpoints.clone(),
        wash: m.wash.clone(),
    }
}

fn fabric_text(fabric: Option<&Fabric>) -> Option<String> {
    let fabric = fabric?;
    if fabric.composition.is_empty() {
        return None;
    }
    Some(
        fabric
            .composition
            .iter()
            .map(|share| {
                format!(
                    "{}% {}",
                    omni_core::js::number_to_string(share.percent),
                    share.fiber
                )
            })
            .collect::<Vec<_>>()
            .join(", "),
    )
}

/// The newest current measurement per dimension and method of `subject`.
fn newest_measurements(state: &LedgerState, subject: &str) -> Vec<StylistMeasurement> {
    let mut newest: BTreeMap<(String, String), (&Record, &MeasurementRecord)> = BTreeMap::new();
    for record in state.current_records() {
        let RecordBody::Measurement(m) = &record.body else {
            continue;
        };
        if m.subject.id.as_ref().is_none_or(|s| s.as_str() != subject) {
            continue;
        }
        let key = (m.dimension.as_str().to_owned(), enum_text(&m.method));
        let newer = newest.get(&key).is_none_or(|(r, old)| {
            (m.observed_on.as_str(), record.ledger_revision)
                > (old.observed_on.as_str(), r.ledger_revision)
        });
        if newer {
            newest.insert(key, (record, m));
        }
    }
    newest
        .into_values()
        .map(|(record, m)| shown(&measurement_ref(record, m)))
        .collect()
}

fn piece(state: &LedgerState, record: &Record) -> Option<StylistPiece> {
    let RecordBody::Piece(p) = &record.body else {
        return None;
    };
    let id = record.id.as_str();
    let variant = state.piece_variant(id).map(|(_, v)| v);
    let events: Vec<_> = state.piece_events(id).collect();
    Some(StylistPiece {
        id: id.to_owned(),
        label: p.label.clone(),
        category: p.category.as_str().to_owned(),
        status: state.piece_status(id).as_str().to_owned(),
        brand: variant.map(|v| v.brand.clone()),
        model: variant.map(|v| v.model.clone()),
        size_label: variant.map(|v| v.size_label.clone()),
        colour: variant.and_then(|v| v.colour.clone()),
        fabric: fabric_text(variant.and_then(|v| v.fabric.as_ref())),
        fit: enum_text(&fit::piece_fit_evidence(state, id).fit),
        reference_role: p.reference.as_ref().map(|r| enum_text(&r.role)),
        reference_regions: p
            .reference
            .as_ref()
            .map(|r| r.regions.iter().map(|g| g.as_str().to_owned()).collect())
            .unwrap_or_default(),
        washes: count(
            events
                .iter()
                .filter(|e| matches!(e.envelope.event, EventBody::Wash(_)))
                .count(),
        ),
        alterations: count(
            events
                .iter()
                .filter(|e| matches!(e.envelope.event, EventBody::Alteration(_)))
                .count(),
        ),
        measurements: newest_measurements(state, id),
        last_event_on: events.iter().map(|e| e.occurred_on.clone()).max(),
        return_by: state
            .acquisition(id)
            .and_then(|(_, a)| a.order.as_ref())
            .and_then(|o| o.return_by.clone()),
    })
}

fn case(entry: &HistoryEntry, state: &LedgerState) -> StylistCase {
    let subject = match (&entry.brand, &entry.model, &entry.size_label) {
        (Some(brand), Some(model), Some(size)) => format!("{brand} {model}, {size}"),
        _ => entry
            .piece_id
            .as_deref()
            .and_then(|id| state.piece(id))
            .map_or_else(|| "Unknown piece".to_owned(), |p| p.label.clone()),
    };
    StylistCase {
        kind: entry.kind.clone(),
        occurred_on: entry.occurred_on.clone(),
        subject,
        category: entry.category.clone(),
        detail: entry.detail.clone(),
        regions: entry
            .regions
            .iter()
            .map(|r| StylistRegion {
                region: r.region.clone(),
                verdict: r.verdict.clone(),
            })
            .collect(),
    }
}

fn prediction(state: &LedgerState, record: &Record) -> Option<StylistPrediction> {
    let RecordBody::Prediction(p) = &record.body else {
        return None;
    };
    let PredictionRecord {
        variant_id,
        recommended_size,
        ..
    } = p;
    let variant = state.variant(variant_id.as_str());
    let derived = record.derived.as_ref();
    let text = |key: &str| {
        derived
            .and_then(|d| d.get(key))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    let outcomes = state
        .current_events()
        .filter_map(|e| match &e.envelope.event {
            EventBody::Outcome(o) if o.prediction_id == record.id => Some((e, o)),
            _ => None,
        })
        .map(|(event, outcome)| StylistOutcome {
            stage: enum_text(&outcome.stage),
            result: enum_text(&outcome.result),
            occurred_on: event.occurred_on.clone(),
            scores: event
                .derived
                .as_ref()
                .and_then(|d| d.get("forecastComparison"))
                .and_then(|c| serde_json::from_value::<Vec<fit::RegionComparison>>(c.clone()).ok())
                .unwrap_or_default()
                .into_iter()
                .map(|c| StylistScore {
                    region: c.region,
                    expected: c.expected.map(|v| enum_text(&v)),
                    actual: c.actual.map(|v| enum_text(&v)),
                    matched: enum_text(&c.matched),
                })
                .collect(),
        })
        .collect();
    Some(StylistPrediction {
        id: record.id.as_str().to_owned(),
        frozen_at: iso(record.created_at),
        subject: variant.map_or_else(
            || format!("{variant_id}, {recommended_size}"),
            |v| format!("{} {}, {}", v.brand, v.model, recommended_size),
        ),
        category: variant.map_or_else(String::new, |v| v.category.as_str().to_owned()),
        decision: enum_text(&p.decision),
        basis: enum_text(&p.basis),
        computed_basis: text("computedBasis"),
        return_risk: enum_text(&p.return_risk.level),
        failure_mode: p.likely_failure_mode.description.clone(),
        expectations: p
            .region_expectations
            .iter()
            .map(|e| StylistRegion {
                region: e.region.as_str().to_owned(),
                verdict: enum_text(&e.expected),
            })
            .collect(),
        evidence_gaps: p.evidence_gaps.clone(),
        context_verified: derived
            .and_then(|d| d.pointer("/context/verified"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        runbook_version: text("runbookVersion"),
        outcomes,
        superseded: state.record_superseded_by(record.id.as_str()).is_some(),
    })
}

fn rule(state: &LedgerState, record: &Record, context: &ContextOutput) -> Option<StylistRule> {
    let RecordBody::Rule(r) = &record.body else {
        return None;
    };
    let scope: Vec<String> = [
        r.scope.brand.clone(),
        r.scope.model.clone(),
        r.scope.category.as_ref().map(|c| c.as_str().to_owned()),
        r.scope.size_label.clone(),
        r.scope.region.as_ref().map(|g| g.as_str().to_owned()),
        r.scope.fiber.as_ref().map(|f| f.as_str().to_owned()),
    ]
    .into_iter()
    .flatten()
    .collect();
    let unchecked = context
        .rules
        .active
        .iter()
        .chain(&context.rules.proposed)
        .find(|out| out.rule.id == record.id.as_str())
        .map_or(0, |out| out.unchecked_cases.len());
    Some(StylistRule {
        id: record.id.as_str().to_owned(),
        statement: r.statement.clone(),
        status: enum_text(&r.status),
        basis: enum_text(&r.basis),
        scope: scope.join(" · "),
        in_scope_cases: count(fit::cases_in_scope(state, &r.scope).len()),
        unchecked_cases: count(unchecked),
        needs_review: context
            .rules
            .needs_review
            .iter()
            .any(|id| id == record.id.as_str()),
    })
}

fn archive(m: ArchiveManifest) -> StylistArchive {
    let as_f64 = |n: u64| u32::try_from(n).map_or(f64::MAX, f64::from);
    StylistArchive {
        archive_id: m.archive_id,
        reason: m.reason,
        ledger_revision: as_f64(m.ledger_revision),
        exported_at: m.exported_at,
        includes_bytes: m.includes_bytes,
        file_bytes: as_f64(m.file_bytes),
        evidence: as_f64(m.counts.evidence),
    }
}

/// Builds the overview of `state` from its context.
pub fn overview(
    state: &LedgerState,
    context: &ContextOutput,
    namespaces: Vec<(String, u64)>,
    archives: Vec<ArchiveManifest>,
) -> StylistOverview {
    let as_f64 = |n: u64| u32::try_from(n).map_or(f64::MAX, f64::from);
    let mut namespaces: Vec<StylistNamespace> = namespaces
        .into_iter()
        .map(|(namespace, revision)| StylistNamespace {
            fixture: namespace.starts_with("fixture-"),
            namespace,
            revision: as_f64(revision),
        })
        .collect();
    if !namespaces.iter().any(|n| n.namespace == "real") {
        namespaces.insert(
            0,
            StylistNamespace {
                namespace: "real".to_owned(),
                revision: 0.0,
                fixture: false,
            },
        );
    }
    namespaces.sort_by(|a, b| (a.fixture, &a.namespace).cmp(&(b.fixture, &b.namespace)));
    let kind_count = |kind: RecordKind| {
        context
            .counts
            .records
            .get(kind.as_str())
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
    };
    let snapshot: Option<ProfileSnapshotRecord> = context
        .profile
        .latest_snapshot
        .as_ref()
        .and_then(|item| serde_json::from_value(Value::Object(item.data.clone())).ok());
    let profile = StylistProfile {
        observed_on: snapshot.as_ref().map(|s| s.observed_on.clone()),
        attributes: snapshot
            .as_ref()
            .map(|s| {
                s.attributes
                    .iter()
                    .map(|a| StylistAttribute {
                        name: a.name.as_str().to_owned(),
                        value: match (a.number, a.unit, &a.text) {
                            (Some(n), Some(unit), _) => format!(
                                "{} {}",
                                omni_core::js::number_to_string(n),
                                enum_text(&unit)
                            ),
                            (Some(n), None, _) => omni_core::js::number_to_string(n),
                            (None, _, Some(text)) => text.clone(),
                            (None, _, None) => String::new(),
                        },
                        certainty: enum_text(&a.certainty),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        sensitivities: snapshot
            .as_ref()
            .map(|s| {
                s.fit_sensitivities
                    .iter()
                    .map(|f| format!("{}: {}", f.region, f.note))
                    .collect()
            })
            .unwrap_or_default(),
        body: context
            .profile
            .body_measurements
            .iter()
            .map(|b| StylistBodyMeasurement {
                latest: shown(&b.latest),
                certainty: b.certainty.clone(),
                source: b.source.clone(),
                earlier: b.earlier.iter().map(shown).collect(),
                stale: b.stale,
            })
            .collect(),
    };
    let mut wardrobe: Vec<StylistPiece> = state
        .current_records()
        .filter_map(|r| piece(state, r))
        .collect();
    wardrobe.sort_by(|a, b| {
        (&a.category, b.reference_role.is_some(), &a.label).cmp(&(
            &b.category,
            a.reference_role.is_some(),
            &b.label,
        ))
    });
    let mut predictions: Vec<(i64, StylistPrediction)> = state
        .records
        .values()
        .filter_map(|r| prediction(state, r).map(|p| (r.created_at, p)))
        .collect();
    predictions.sort_by_key(|p| std::cmp::Reverse(p.0));
    // Open needs first, then by priority (high, medium, low) and category.
    let mut needs: Vec<(bool, Priority, StylistNeed)> = state
        .current_records()
        .filter_map(|r| match &r.body {
            RecordBody::Need(n) => Some((
                n.status != NeedStatus::Open,
                n.priority,
                StylistNeed {
                    id: r.id.as_str().to_owned(),
                    category: n.category.as_str().to_owned(),
                    reason: enum_text(&n.reason),
                    priority: enum_text(&n.priority),
                    description: n.description.clone(),
                    status: enum_text(&n.status),
                    review_after: n.review_after.clone(),
                },
            )),
            _ => None,
        })
        .collect();
    let rank = |p: Priority| match p {
        Priority::High => 0,
        Priority::Medium => 1,
        Priority::Low => 2,
    };
    needs.sort_by(|a, b| (a.0, rank(a.1), &a.2.category).cmp(&(b.0, rank(b.1), &b.2.category)));
    let needs: Vec<StylistNeed> = needs.into_iter().map(|(_, _, need)| need).collect();
    let changes: BTreeMap<&str, &str> = context
        .preferences
        .recent_changes
        .iter()
        .map(|c| (c.id.as_str(), c.previous_statement.as_str()))
        .collect();
    let history = &context.history;
    StylistOverview {
        namespace: context.snapshot.namespace.clone(),
        namespaces,
        ledger_revision: as_f64(context.snapshot.ledger_revision),
        generated_at: context.snapshot.generated_at.clone(),
        today: context.snapshot.today.clone(),
        runbook: StylistRunbook {
            version: context.runbook.version.clone(),
            sha256: context.runbook.sha256.clone(),
            checklist: context.runbook.checklist.clone(),
        },
        counts: StylistCounts {
            variants: kind_count(RecordKind::Variant),
            pieces: kind_count(RecordKind::Piece),
            measurements: kind_count(RecordKind::Measurement),
            profile_snapshots: kind_count(RecordKind::ProfileSnapshot),
            preferences: kind_count(RecordKind::Preference),
            predictions: kind_count(RecordKind::Prediction),
            rules: kind_count(RecordKind::Rule),
            needs: kind_count(RecordKind::Need),
            events: as_f64(context.counts.events),
            evidence: as_f64(context.counts.evidence),
        },
        warnings: context
            .warnings
            .iter()
            .map(|w| StylistWarning {
                code: w.code.clone(),
                severity: w.severity.clone(),
                message: w.message.clone(),
            })
            .collect(),
        due: context
            .due
            .iter()
            .map(|d| StylistDue {
                kind: d.kind.clone(),
                id: d.id.clone(),
                label: d.label.clone(),
                due_on: d.due_on.clone(),
                days_left: d.days_left.map(|n| n as f64),
                detail: d.detail.clone(),
            })
            .collect(),
        contradictions: context
            .contradictions
            .iter()
            .map(|c| StylistContradiction {
                source: c.source.clone(),
                kind: c.kind.clone(),
                description: c.description.clone(),
            })
            .collect(),
        profile,
        wardrobe,
        history: StylistHistory {
            successes: history.successes.iter().map(|e| case(e, state)).collect(),
            failures: history.failures.iter().map(|e| case(e, state)).collect(),
            unresolved: history.unresolved.iter().map(|e| case(e, state)).collect(),
            success_total: as_f64(history.totals.successes),
            failure_total: as_f64(history.totals.failures),
            unresolved_total: as_f64(history.totals.unresolved),
        },
        predictions: predictions.into_iter().map(|(_, p)| p).collect(),
        rules: state
            .current_records()
            .filter_map(|r| rule(state, r, context))
            .collect(),
        preferences: context
            .preferences
            .current
            .iter()
            .filter_map(|item| {
                let RecordBody::Preference(p) = &state.record(&item.id)?.body else {
                    return None;
                };
                Some(StylistPreference {
                    id: item.id.clone(),
                    category: p.category.as_str().to_owned(),
                    aspect: enum_text(&p.aspect),
                    region: p.region.as_ref().map(|g| g.as_str().to_owned()),
                    statement: p.statement.clone(),
                    strength: enum_text(&p.strength),
                    valid_from: p.valid_from.clone(),
                    previous: changes.get(item.id.as_str()).map(|s| (*s).to_owned()),
                })
            })
            .collect(),
        needs,
        archives: archives.into_iter().map(archive).collect(),
    }
}

#[derive(Deserialize)]
struct OverviewQuery {
    namespace: Option<String>,
}

/// `GET /api/stylist`, state applied.
pub fn router(stylist: Stylist) -> Router {
    Router::new()
        .route("/api/stylist", get(get_overview))
        .with_state(stylist)
}

async fn get_overview(
    State(stylist): State<Stylist>,
    Query(query): Query<OverviewQuery>,
) -> Result<Response, ApiError> {
    let namespace = Namespace::parse(query.namespace.as_deref().unwrap_or("real"))
        .map_err(ApiError::bad_request)?;
    let overview = stylist
        .overview(&namespace)
        .await
        .map_err(|e| ApiError::internal(e.tool_text()))?;
    let value = serde_json::to_value(&overview).map_err(ApiError::internal)?;
    Ok(json_response(
        axum::http::StatusCode::OK,
        omni_core::js::json_stringify(&value),
    ))
}

impl Stylist {
    /// The web overview of `namespace`.
    pub async fn overview(&self, namespace: &Namespace) -> Result<StylistOverview, StylistError> {
        let state = self.state(namespace, None).await?;
        let namespaces = self
            .store
            .read(|docs| Ok(ledger::namespaces(docs.connection())))
            .await??;
        let input = ContextInput {
            namespace: namespace.clone(),
            focus: None,
            as_of_revision: None,
            include_runbook: Some(false),
            runbook_version: None,
            limit: Some(50),
            context_token: None,
        };
        let context = context::build(&state, &input, self.today(), self.clock.now_ms())?;
        let archives = self.list_archives(Some(namespace), 10).await?.archives;
        Ok(overview(&state, &context, namespaces, archives))
    }
}
