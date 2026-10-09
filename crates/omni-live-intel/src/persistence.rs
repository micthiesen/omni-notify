//! Livestream intelligence persistence (`persistence.ts`).

use omni_store::cbor::Extra;
use omni_store::entity::{EntityOps as _, EntityWrite as _, UpsertOpts};
use omni_store::{DocOps as _, Store, StoreError};

use crate::types::{
    EventStatus, LivestreamDiagnosticsData, LivestreamEventKind, LivestreamFeedbackData,
    LivestreamFeedbackVerdict, LivestreamIntelligenceData, LivestreamIntelligenceEventData,
    Metrics, PipelineStage, StageDiagnostic, metric_string,
};

const MAX_TIMELINE_EVENTS: u64 = 3_000;
const PRUNE_BATCH_SIZE: usize = 250;
pub const DESTINY_CONFIRMED_EVENT_TITLE: &str = "Destiny confirmed as a live participant";

pub async fn get_intelligence(
    store: &Store,
    streamer_id: &str,
) -> Result<Option<LivestreamIntelligenceData>, StoreError> {
    let key = streamer_id.to_owned();
    store
        .read(move |docs| docs.get::<LivestreamIntelligenceData>(&key))
        .await
}

pub async fn save_intelligence(
    store: &Store,
    data: LivestreamIntelligenceData,
) -> Result<(), StoreError> {
    store
        .write(move |tx| tx.upsert(&data, UpsertOpts::default()))
        .await
}

pub async fn get_diagnostics(
    store: &Store,
    streamer_id: &str,
) -> Result<Option<LivestreamDiagnosticsData>, StoreError> {
    let key = streamer_id.to_owned();
    store
        .read(move |docs| docs.get::<LivestreamDiagnosticsData>(&key))
        .await
}

/// Sets one stage; stages of the same session are merged, a new session starts fresh.
pub async fn update_stage(
    store: &Store,
    streamer_id: &str,
    session_started_at: Option<i64>,
    stage: PipelineStage,
    value: StageDiagnostic,
) -> Result<LivestreamDiagnosticsData, StoreError> {
    let key = streamer_id.to_owned();
    store
        .write(move |tx| {
            let previous = tx.get::<LivestreamDiagnosticsData>(&key)?;
            let mut stages = match previous {
                Some(previous) if previous.session_started_at == session_started_at => {
                    previous.stages
                }
                _ => Default::default(),
            };
            stages.insert(stage.as_str().to_owned(), value);
            let next = LivestreamDiagnosticsData {
                streamer_id: key,
                session_started_at,
                stages,
                updated_at: tx.now_ms(),
                extra: Extra::new(),
            };
            tx.upsert(&next, UpsertOpts::default())?;
            Ok(next)
        })
        .await
}

/// `RecordLivestreamEventInput`: id and time default to a new UUID and now.
#[derive(Clone, Debug, PartialEq)]
pub struct NewLivestreamEvent {
    pub streamer_id: String,
    pub session_started_at: Option<i64>,
    pub kind: LivestreamEventKind,
    pub status: EventStatus,
    pub title: String,
    pub detail: Option<String>,
    pub duration_ms: Option<i64>,
    pub cost_cents: Option<f64>,
    pub metrics: Option<Metrics>,
    pub event_id: Option<String>,
    pub created_at: Option<i64>,
}

impl NewLivestreamEvent {
    pub fn new(
        streamer_id: impl Into<String>,
        session_started_at: Option<i64>,
        kind: LivestreamEventKind,
        status: EventStatus,
        title: impl Into<String>,
    ) -> Self {
        Self {
            streamer_id: streamer_id.into(),
            session_started_at,
            kind,
            status,
            title: title.into(),
            detail: None,
            duration_ms: None,
            cost_cents: None,
            metrics: None,
            event_id: None,
            created_at: None,
        }
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn maybe_detail(mut self, detail: Option<String>) -> Self {
        self.detail = detail;
        self
    }

    pub fn duration_ms(mut self, duration_ms: i64) -> Self {
        self.duration_ms = Some(duration_ms);
        self
    }

    pub fn cost_cents(mut self, cost_cents: f64) -> Self {
        self.cost_cents = Some(cost_cents);
        self
    }

    pub fn metrics(mut self, metrics: Metrics) -> Self {
        self.metrics = Some(metrics);
        self
    }

    pub fn created_at(mut self, created_at: i64) -> Self {
        self.created_at = Some(created_at);
        self
    }
}

/// Appends a timeline event; above 3,000 events the oldest 250 are pruned.
pub async fn record_event(
    store: &Store,
    input: NewLivestreamEvent,
) -> Result<LivestreamIntelligenceEventData, StoreError> {
    store
        .write(move |tx| {
            let event = LivestreamIntelligenceEventData {
                streamer_id: input.streamer_id,
                session_started_at: input.session_started_at,
                kind: input.kind,
                status: input.status,
                title: input.title,
                detail: input.detail,
                duration_ms: input.duration_ms,
                cost_cents: input.cost_cents,
                metrics: input.metrics,
                event_id: input.event_id.unwrap_or_else(omni_core::ids::uuid_v4),
                created_at: input.created_at.unwrap_or_else(|| tx.now_ms()),
                extra: Extra::new(),
            };
            tx.upsert(&event, UpsertOpts::default())?;
            if tx.count::<LivestreamIntelligenceEventData>()? > MAX_TIMELINE_EVENTS {
                let mut all = tx.get_all::<LivestreamIntelligenceEventData>()?;
                all.sort_by_key(|e| e.created_at);
                for oldest in all.into_iter().take(PRUNE_BATCH_SIZE) {
                    tx.delete::<LivestreamIntelligenceEventData>(&oldest.event_id)?;
                }
            }
            Ok(event)
        })
        .await
}

/// Newest first, optionally for one streamer; `limit` is clamped to 1..=200.
pub async fn get_events(
    store: &Store,
    streamer_id: Option<&str>,
    limit: usize,
) -> Result<Vec<LivestreamIntelligenceEventData>, StoreError> {
    let filter = streamer_id.map(str::to_owned);
    let mut events = store
        .read(|docs| docs.get_all::<LivestreamIntelligenceEventData>())
        .await?;
    events.retain(|e| filter.as_deref().is_none_or(|id| e.streamer_id == id));
    events.sort_by_key(|e| std::cmp::Reverse(e.created_at));
    events.truncate(limit.clamp(1, 200));
    Ok(events)
}

/// The newest durable Destiny confirmation for the session.
pub async fn get_latest_destiny_confirmation(
    store: &Store,
    streamer_id: &str,
    session_started_at: i64,
) -> Result<Option<LivestreamIntelligenceEventData>, StoreError> {
    let id = streamer_id.to_owned();
    let mut events: Vec<LivestreamIntelligenceEventData> = store
        .read(|docs| docs.get_all::<LivestreamIntelligenceEventData>())
        .await?
        .into_iter()
        .filter(|e| {
            e.streamer_id == id
                && e.session_started_at == Some(session_started_at)
                && e.kind == LivestreamEventKind::Voice
                && e.status == EventStatus::Success
                && e.title == DESTINY_CONFIRMED_EVENT_TITLE
        })
        .collect();
    events.sort_by_key(|e| std::cmp::Reverse(e.created_at));
    Ok(events.into_iter().next())
}

/// Records feedback only for the streamer's latest alert; `None` when stale.
pub async fn record_feedback(
    store: &Store,
    streamer_id: &str,
    alert_id: &str,
    verdict: LivestreamFeedbackVerdict,
    note: Option<&str>,
) -> Result<Option<LivestreamFeedbackData>, StoreError> {
    let Some(intelligence) = get_intelligence(store, streamer_id).await? else {
        return Ok(None);
    };
    let Some(alert) = intelligence
        .latest_alert
        .as_ref()
        .filter(|alert| alert.alert_id == alert_id)
    else {
        return Ok(None);
    };
    let note = note
        .map(|n| crate::js_math::js_trim(n).to_owned())
        .filter(|n| !n.is_empty());
    let alert_type = alert.alert_type;
    let feedback_note = note.clone();
    let streamer = streamer_id.to_owned();
    let alert_key = alert_id.to_owned();
    let feedback = store
        .write(move |tx| {
            let feedback = LivestreamFeedbackData {
                feedback_id: alert_key.clone(),
                streamer_id: streamer,
                alert_id: alert_key,
                alert_type,
                verdict,
                note: feedback_note,
                created_at: tx.now_ms(),
                extra: Extra::new(),
            };
            tx.upsert(&feedback, UpsertOpts::default())?;
            Ok::<_, StoreError>(feedback)
        })
        .await?;
    let mut metrics = Metrics::new();
    metrics.insert("alertType".to_owned(), metric_string(alert_type.as_str()));
    let event = NewLivestreamEvent::new(
        streamer_id,
        Some(intelligence.session_started_at),
        LivestreamEventKind::Feedback,
        EventStatus::Info,
        format!("Alert marked {}", verdict.as_str().replace('_', " ")),
    )
    .maybe_detail(note)
    .metrics(metrics);
    if let Err(error) = record_event(store, event).await {
        tracing::warn!(target: crate::LOG, %error, "Failed to persist feedback event");
    }
    Ok(Some(feedback))
}

/// `type: verdict (note)` lines, newest first.
pub async fn build_feedback_digest(store: &Store, limit: usize) -> Result<String, StoreError> {
    let mut items = store
        .read(|docs| docs.get_all::<LivestreamFeedbackData>())
        .await?;
    items.sort_by_key(|e| std::cmp::Reverse(e.created_at));
    Ok(items
        .iter()
        .take(limit)
        .map(|item| {
            let note = item
                .note
                .as_deref()
                .filter(|n| !n.is_empty())
                .map(|n| format!(" ({n})"))
                .unwrap_or_default();
            format!(
                "{}: {}{note}",
                item.alert_type.as_str(),
                item.verdict.as_str()
            )
        })
        .collect::<Vec<_>>()
        .join("\n"))
}

/// Row count helper for tests and diagnostics.
pub async fn count_events(store: &Store) -> Result<u64, StoreError> {
    store
        .read(|docs| docs.count_by_entity("livestream-intelligence-event"))
        .await
}
