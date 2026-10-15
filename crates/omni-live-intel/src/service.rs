//! The livestream intelligence service: viewer anomalies,
//! Destiny voice presence, rolling summaries and alerts.
//!
//! Rolling summaries listen continuously: each summary target gets one
//! listener that captures back-to-back windows and summarizes each window,
//! speaker-attributed, while the next one records.
//!
//! In-memory state (anomaly samples, voice evidence, cooldowns, schedules)
//! resets on restart. The state mutex is never held across an await.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use indexmap::IndexMap;
use omni_ai::costs::{CostRecorder, NewCostEvent};
use omni_alerts::{Pushover, PushoverChannel, PushoverError, PushoverMessage};
use omni_api::costs::{CostCategory, CostPriceStatus, CostUsage};
use omni_config::Config;
use omni_core::clock::SharedClock;
use omni_core::js::math_round;
use omni_store::cbor::{Extra, JsValue};
use omni_store::entity::EntityOps as _;
use omni_store::{Store, StoreError};
use serde::Serialize;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::LOG;
use crate::alert_policy::{alert_sent_in_session, livestream_alert_confidence_floor};
use crate::anomaly::{AnomalyInput, ViewerAnomalyTracker, compute_relevance, typical_session_peak};
use crate::audio::{AudioError, AudioSource, SAMPLE_RATE};
use crate::classifier::{
    AssessmentError, TranscriptAssessment, TranscriptAssessmentInput, TranscriptClassifier,
    livestream_spend_cents,
};
use crate::js_math::js_min;
use crate::observation::{LiveObservation, Streamer, StreamerTier, viewer_count_for_anomaly};
use crate::persistence::{
    DESTINY_CONFIRMED_EVENT_TITLE, NewLivestreamEvent, build_feedback_digest, get_diagnostics,
    get_intelligence, get_latest_destiny_confirmation, record_event, save_intelligence,
    update_stage,
};
use crate::presence_policy::{VoiceMatchAction, decide_voice_match_action};
use crate::speech::{SpeakerMatch, SpeechEngine, SpeechRecognitionError, TRANSCRIPTION_MODEL};
use crate::summary_text::are_same_livestream_topic;
use crate::transcript::{TranscribedTurn, describe_speakers, format_attributed};
use crate::types::{
    DestinyPresence, EventStatus, LivestreamAlertRecord, LivestreamAlertType, LivestreamChapter,
    LivestreamEventKind, LivestreamIntelligenceData, Metrics, PipelineStage, PipelineStatus,
    PresenceState, RollingSummary, StageDiagnostic, StreamSessionsData, metric_nullable,
    metric_number, metric_string,
};
use crate::voice_evidence::{VoiceEvidenceDecision, VoiceEvidenceTracker};
use crate::voice_targets::select_voice_targets;
use crate::work_queue::{WorkQueue, WorkQueueClosed};

const DESTINY_ID: &str = "destiny";
const PRESENCE_EXPIRY_MS: i64 = 10 * 60_000;
const ALERT_COOLDOWN_MS: i64 = 30 * 60_000;
const MAX_CHAPTERS: usize = 40;
const TRANSCRIPT_EXCERPT_CHARS: usize = 800;
const VERIFY_CONTEXT_SECONDS: u32 = 45;
const DETAIL_MAX_CHARS: usize = 500;
/// Streams listened to at once; further summary targets wait for a slot.
const MAX_LISTENERS: usize = 6;
/// A listener's first window is short so a new target gets a summary soon.
const FIRST_WINDOW_SECONDS: u32 = 120;
/// Shorter captures (a stream ending mid-window) are not summarized.
const MIN_WINDOW_SECONDS: f64 = 30.0;
/// Floor for the configured window length, so windows clear the minimum.
const MIN_LISTEN_WINDOW_SECONDS: u32 = 60;
/// Wait after a failed or short capture before listening again.
const LISTEN_RETRY_DELAY: Duration = Duration::from_secs(120);

#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[error("{0}")]
    Store(#[from] StoreError),
    #[error("{0}")]
    Audio(#[from] AudioError),
    #[error("{0}")]
    Speech(#[from] SpeechRecognitionError),
    #[error("{0}")]
    Assessment(#[from] AssessmentError),
    #[error("{0}")]
    Queue(#[from] WorkQueueClosed),
    #[error("{}", .0.body)]
    Notification(#[from] PushoverError),
}

/// `failureMessage(error).slice(0, 500)`.
fn failure_detail(error: &ServiceError) -> String {
    let message = error.to_string();
    omni_core::js::utf16_slice(&message, 0, DETAIL_MAX_CHARS).into_owned()
}

/// A capture that found the stream not broadcasting. The live check lags the
/// platform when a stream ends, so this is a skip, not a failure event.
fn is_not_live(error: &ServiceError) -> bool {
    matches!(error, ServiceError::Audio(AudioError::NotLive { .. }))
}

/// Whether the stream belongs to Destiny himself (always excluded from voice targets).
pub fn is_destiny_owned_stream(streamer: &Streamer) -> bool {
    std::iter::once(streamer.id.as_str())
        .chain(std::iter::once(streamer.display_name.as_str()))
        .chain(streamer.bindings.iter().map(|b| b.username.as_str()))
        .any(|value| {
            let lowered = value.trim().to_lowercase();
            lowered.strip_prefix('@').unwrap_or(&lowered) == DESTINY_ID
        })
}

/// Runtime knobs (the `LIVESTREAM_*` config values).
#[derive(Clone, Debug, PartialEq)]
pub struct ServiceSettings {
    pub destiny_speaker_threshold: f64,
    pub max_voice_targets: usize,
    pub voice_sample_seconds: u32,
    pub voice_sample_interval_seconds: u32,
    /// Length of each continuously captured summary window.
    pub summary_interval_seconds: u32,
    pub monthly_budget_usd: f64,
    pub tz: jiff::tz::TimeZone,
}

impl ServiceSettings {
    pub fn from_config(config: &Config) -> Self {
        Self {
            destiny_speaker_threshold: config.livestream_destiny_speaker_threshold,
            max_voice_targets: config.livestream_max_voice_targets as usize,
            voice_sample_seconds: config.livestream_voice_sample_seconds,
            voice_sample_interval_seconds: config.livestream_voice_sample_interval_seconds,
            summary_interval_seconds: config.livestream_summary_interval_seconds,
            monthly_budget_usd: config.livestream_monthly_budget_usd,
            tz: jiff::tz::TimeZone::get(&config.tz).unwrap_or(jiff::tz::TimeZone::UTC),
        }
    }

    fn voice_interval_ms(&self) -> i64 {
        i64::from(self.voice_sample_interval_seconds) * 1000
    }

    fn summary_interval_ms(&self) -> i64 {
        i64::from(self.summary_interval_seconds) * 1000
    }
}

/// Everything the service needs; tests substitute capture, speech and classifier.
pub struct ServiceDeps {
    pub store: Store,
    pub clock: SharedClock,
    pub pushover: Pushover,
    pub costs: CostRecorder,
    pub capture: Arc<dyn AudioSource>,
    pub speech: Arc<dyn SpeechEngine>,
    pub classifier: Arc<dyn TranscriptClassifier>,
    pub tracker: TaskTracker,
    pub settings: ServiceSettings,
}

#[derive(Default)]
struct State {
    anomaly: ViewerAnomalyTracker,
    typical_peaks: HashMap<String, (i64, Option<f64>)>,
    pending_voice: HashMap<String, i64>,
    last_voice_sample: HashMap<String, i64>,
    /// Streamers that currently qualify for rolling summaries.
    summary_targets: HashSet<String>,
    /// Streamers with a running listener.
    listening: HashSet<String>,
    voice_evidence: VoiceEvidenceTracker,
    active: IndexMap<String, LiveObservation>,
    alert_times: HashMap<String, i64>,
    voiceprint_warning_logged: bool,
}

struct Inner {
    deps: ServiceDeps,
    capture_queue: WorkQueue,
    speech_queue: WorkQueue,
    /// Rolling-summary diarization and transcription, apart from
    /// `speech_queue` so voice detection never waits behind a window.
    summary_speech_queue: WorkQueue,
    llm_queue: WorkQueue,
    background_queue: WorkQueue,
    listen_queue: WorkQueue,
    shutdown: CancellationToken,
    state: Mutex<State>,
}

/// Queue occupancy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct QueueDiagnostics {
    pub running: usize,
    pub queued: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct RuntimeQueues {
    pub capture: QueueDiagnostics,
    pub speech: QueueDiagnostics,
    pub llm: QueueDiagnostics,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeBudget {
    pub spent_cents: f64,
    pub limit_cents: f64,
    pub remaining_cents: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeIntervals {
    pub voice_seconds: u32,
    pub summary_seconds: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeDiagnostics {
    pub enabled: bool,
    pub voiceprint_loaded: bool,
    pub model: &'static str,
    pub queues: RuntimeQueues,
    pub active_stream_count: usize,
    pub active_voice_target_count: usize,
    pub budget: RuntimeBudget,
    pub intervals: RuntimeIntervals,
}

/// Clears a pending voice marker when the job ends or is interrupted, if it
/// is still the job's own session.
struct PendingGuard {
    inner: Arc<Inner>,
    streamer_id: String,
    session_started_at: i64,
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        let mut state = lock(&self.inner.state);
        if state.pending_voice.get(&self.streamer_id) == Some(&self.session_started_at) {
            state.pending_voice.remove(&self.streamer_id);
        }
    }
}

/// Clears the listening marker when a listener ends or is interrupted.
struct ListenerGuard {
    inner: Arc<Inner>,
    streamer_id: String,
}

impl Drop for ListenerGuard {
    fn drop(&mut self) {
        lock(&self.inner.state).listening.remove(&self.streamer_id);
    }
}

/// A window's speaker-attributed transcript.
struct AttributedTranscript {
    text: String,
    /// Each speaker's share, `Destiny 62%, Speaker 1 30%`.
    speakers: String,
    speaker_count: usize,
    /// Transcribed characters without speaker labels.
    spoken_chars: usize,
}

/// One captured summary window awaiting transcription and assessment.
struct SummaryWindow {
    observation: LiveObservation,
    started_at: i64,
    duration_seconds: f64,
    samples: Arc<Vec<f32>>,
}

fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn new_state(observation: &LiveObservation, now: i64) -> LivestreamIntelligenceData {
    let primary = observation.streamer.tier == StreamerTier::Primary;
    LivestreamIntelligenceData {
        streamer_id: observation.streamer.id.clone(),
        session_started_at: observation.session_started_at(),
        relevance_score: if primary { 40.0 } else { 15.0 },
        relevance_reasons: if primary {
            vec!["primary channel".to_owned()]
        } else {
            Vec::new()
        },
        chapters: Vec::new(),
        updated_at: now,
        semantic: None,
        trend: None,
        summary: None,
        destiny_presence: None,
        latest_alert: None,
        alerted_at_by_type: None,
        extra: Extra::new(),
    }
}

fn js_number(value: Option<&JsValue>) -> f64 {
    match value {
        None | Some(JsValue::Undefined | JsValue::Null) => 0.0,
        Some(JsValue::Float(f) | JsValue::Date(f)) => *f,
        #[allow(clippy::cast_precision_loss)]
        Some(JsValue::Int(i)) => *i as f64,
        Some(JsValue::Bool(b)) => f64::from(u8::from(*b)),
        Some(JsValue::String(s)) => omni_core::js::string_to_number(s),
        Some(_) => f64::NAN,
    }
}

fn metrics<const N: usize>(entries: [(&str, JsValue); N]) -> Metrics {
    entries
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect()
}

fn stage(status: PipelineStatus) -> StageDiagnostic {
    StageDiagnostic::new(status)
}

fn percent(value: f64) -> String {
    omni_core::js::number_to_string(math_round(value * 100.0))
}

/// An alert to consider sending.
struct AlertCandidate<'a> {
    observation: &'a LiveObservation,
    alert_type: LivestreamAlertType,
    title: String,
    message: String,
    reason: String,
    confidence: f64,
}

/// The service; cheap to clone.
#[derive(Clone)]
pub struct LivestreamIntelligenceService {
    inner: Arc<Inner>,
}

impl LivestreamIntelligenceService {
    pub fn new(deps: ServiceDeps) -> Self {
        let tracker = deps.tracker.clone();
        Self {
            inner: Arc::new(Inner {
                capture_queue: WorkQueue::new("livestream-capture", 2, tracker.clone()),
                speech_queue: WorkQueue::new("livestream-speech", 1, tracker.clone()),
                summary_speech_queue: WorkQueue::new(
                    "livestream-summary-speech",
                    1,
                    tracker.clone(),
                ),
                llm_queue: WorkQueue::new("livestream-llm", 1, tracker.clone()),
                background_queue: WorkQueue::new("livestream-background", 8, tracker.clone()),
                listen_queue: WorkQueue::new("livestream-listen", MAX_LISTENERS, tracker),
                shutdown: CancellationToken::new(),
                deps,
                state: Mutex::new(State::default()),
            }),
        }
    }

    fn deps(&self) -> &ServiceDeps {
        &self.inner.deps
    }

    fn settings(&self) -> &ServiceSettings {
        &self.inner.deps.settings
    }

    fn now(&self) -> i64 {
        self.deps().clock.now_ms()
    }

    fn state(&self) -> MutexGuard<'_, State> {
        lock(&self.inner.state)
    }

    /// Whether the streamer has a live observation this session.
    pub fn is_active(&self, streamer_id: &str) -> bool {
        self.state().active.contains_key(streamer_id)
    }

    /// Ids with a live observation.
    pub fn active_ids(&self) -> Vec<String> {
        self.state().active.keys().cloned().collect()
    }

    pub async fn runtime_diagnostics(&self) -> Result<RuntimeDiagnostics, StoreError> {
        let settings = self.settings();
        let spent_cents =
            livestream_spend_cents(&self.deps().store, self.now(), &settings.tz).await?;
        let limit_cents = settings.monthly_budget_usd * 100.0;
        let queue = |q: &WorkQueue| QueueDiagnostics {
            running: q.running(),
            queued: q.queued(),
        };
        let (active_stream_count, active_voice_target_count) = {
            let state = self.state();
            let targets = state
                .active
                .values()
                .filter(|o| self.is_voice_target(o))
                .count();
            (state.active.len(), targets)
        };
        Ok(RuntimeDiagnostics {
            enabled: true,
            voiceprint_loaded: self.deps().speech.has_voiceprint(),
            model: TRANSCRIPTION_MODEL,
            queues: RuntimeQueues {
                capture: queue(&self.inner.capture_queue),
                speech: queue(&self.inner.speech_queue),
                llm: queue(&self.inner.llm_queue),
            },
            active_stream_count,
            active_voice_target_count,
            budget: RuntimeBudget {
                spent_cents,
                limit_cents,
                remaining_cents: (limit_cents - spent_cents).max(0.0),
            },
            intervals: RuntimeIntervals {
                voice_seconds: settings.voice_sample_interval_seconds,
                summary_seconds: settings.summary_interval_seconds,
            },
        })
    }

    async fn record(&self, event: NewLivestreamEvent) {
        if let Err(error) = record_event(&self.deps().store, event).await {
            tracing::warn!(target: LOG, "Failed to persist intelligence event: {error}");
        }
    }

    async fn set_stage(
        &self,
        streamer_id: &str,
        session_started_at: i64,
        pipeline_stage: PipelineStage,
        value: StageDiagnostic,
    ) {
        if let Err(error) = update_stage(
            &self.deps().store,
            streamer_id,
            Some(session_started_at),
            pipeline_stage,
            value,
        )
        .await
        {
            tracing::warn!(
                target: LOG,
                "Failed to persist {} diagnostics: {error}",
                pipeline_stage.as_str()
            );
        }
    }

    /// Cached once per session; history only changes when a session ends. A
    /// failed read is not cached, so the next observation retries it.
    async fn typical_peak(
        &self,
        streamer_id: &str,
        session_started_at: i64,
        now: i64,
    ) -> Option<f64> {
        if let Some((session, peak)) = self.state().typical_peaks.get(streamer_id)
            && *session == session_started_at
        {
            return *peak;
        }
        let key = streamer_id.to_owned();
        match self
            .deps()
            .store
            .read(move |docs| docs.get::<StreamSessionsData>(&key))
            .await
        {
            Ok(data) => {
                let sessions = data.map(|d| d.sessions).unwrap_or_default();
                let peak = typical_session_peak(&sessions, now);
                self.state()
                    .typical_peaks
                    .insert(streamer_id.to_owned(), (session_started_at, peak));
                peak
            }
            Err(error) => {
                tracing::warn!(
                    target: LOG,
                    "Typical viewer peak unavailable for {streamer_id}: {error}"
                );
                None
            }
        }
    }

    fn is_voice_target(&self, observation: &LiveObservation) -> bool {
        self.deps().speech.has_voiceprint()
            && !is_destiny_owned_stream(&observation.streamer)
            && observation.streamer.dgg.is_some()
    }

    fn is_summary_target(
        observation: &LiveObservation,
        state: &LivestreamIntelligenceData,
    ) -> bool {
        observation.streamer.tier == StreamerTier::Primary
            || state
                .destiny_presence
                .as_ref()
                .is_some_and(|p| p.state == PresenceState::Confirmed)
            || state.trend.as_ref().is_some_and(|t| t.anomalous)
            || state.relevance_score >= 80.0
    }

    fn voice_due(&self, streamer_id: &str, now: i64) -> bool {
        let state = self.state();
        !state.pending_voice.contains_key(streamer_id)
            && now
                - state
                    .last_voice_sample
                    .get(streamer_id)
                    .copied()
                    .unwrap_or(0)
                >= self.settings().voice_interval_ms()
    }

    async fn current_session(
        &self,
        observation: &LiveObservation,
    ) -> Result<Option<LivestreamIntelligenceData>, StoreError> {
        if !self.is_active(&observation.streamer.id) {
            return Ok(None);
        }
        Ok(
            get_intelligence(&self.deps().store, &observation.streamer.id)
                .await?
                .filter(|current| current.session_started_at == observation.session_started_at()),
        )
    }

    /// One live observation of a streamer (every polled tick).
    pub async fn observe_live(&self, observation: LiveObservation) -> Result<(), ServiceError> {
        let warn_missing_voiceprint = {
            let mut state = self.state();
            let warn = !self.deps().speech.has_voiceprint() && !state.voiceprint_warning_logged;
            state.voiceprint_warning_logged |= warn;
            warn
        };
        if warn_missing_voiceprint {
            tracing::warn!(
                target: LOG,
                "Livestream intelligence started without a Destiny voiceprint; summaries are enabled but guest detection is disabled"
            );
        }
        let now = self.now();
        let id = observation.streamer.id.clone();
        self.state().active.insert(id.clone(), observation.clone());
        let store = &self.deps().store;
        let previous = get_intelligence(store, &id).await?;
        let session_started_at = observation.session_started_at();
        let is_new_session = previous
            .as_ref()
            .is_none_or(|p| p.session_started_at != session_started_at);
        if is_new_session {
            self.state().anomaly.clear(&id);
        }
        let mut state = match &previous {
            Some(previous) if !is_new_session => previous.clone(),
            _ => new_state(&observation, now),
        };
        let typical_peak = self.typical_peak(&id, session_started_at, now).await;
        let primary = &observation.status.primary;
        let trend = self.state().anomaly.observe(AnomalyInput {
            streamer_id: &id,
            viewers: viewer_count_for_anomaly(&observation.status),
            dgg_viewers: observation.streamer.dgg.and_then(|d| d.viewers),
            session_started_at,
            source_key: Some(format!(
                "{}:{}",
                primary.platform.as_str(),
                primary.username
            )),
            total_viewers: observation.status.viewer_count,
            typical_peak,
            now,
        });
        if state
            .destiny_presence
            .as_ref()
            .is_some_and(|p| now - p.detected_at > PRESENCE_EXPIRY_MS)
        {
            state.destiny_presence = None;
        }
        // Title-only model classification mostly paraphrased the visible title.
        state.semantic = None;
        let destiny_confirmed = state
            .destiny_presence
            .as_ref()
            .is_some_and(|p| p.state == PresenceState::Confirmed);
        let (score, reasons) =
            compute_relevance(&observation.streamer, None, Some(&trend), destiny_confirmed);
        state.trend = Some(trend.clone());
        state.relevance_score = score;
        state.relevance_reasons = reasons;
        state.updated_at = now;
        save_intelligence(store, state.clone()).await?;

        let previous_diagnostics = get_diagnostics(store, &id).await?;
        if is_new_session {
            self.record(
                NewLivestreamEvent::new(
                    &id,
                    Some(session_started_at),
                    LivestreamEventKind::Session,
                    EventStatus::Info,
                    "Live session started",
                )
                .detail(observation.status.primary_title.clone()),
            )
            .await;
        }
        if is_new_session
            || previous_diagnostics
                .as_ref()
                .is_none_or(|d| d.session_started_at != Some(session_started_at))
        {
            self.reset_stages(&observation, &state, now).await;
        }

        if trend.anomalous && score >= 70.0 {
            if !previous
                .as_ref()
                .and_then(|p| p.trend.as_ref())
                .is_some_and(|t| t.anomalous)
            {
                self.record(
                    NewLivestreamEvent::new(
                        &id,
                        Some(session_started_at),
                        LivestreamEventKind::Anomaly,
                        EventStatus::Warning,
                        "Viewer surge detected",
                    )
                    .maybe_detail(trend.reason.clone())
                    .metrics(metrics([
                        ("percentChange", metric_number(trend.percent_change)),
                        ("viewersPerMinute", metric_number(trend.viewers_per_minute)),
                        (
                            "dggPercentChange",
                            metric_nullable(trend.dgg_percent_change),
                        ),
                    ])),
                )
                .await;
            }
            let service = self.clone();
            let surge = observation.clone();
            let reason = trend.reason.clone();
            self.inner.background_queue.fork(async move {
                let candidate = AlertCandidate {
                    observation: &surge,
                    alert_type: LivestreamAlertType::ViewerSurge,
                    title: format!("{} is surging", surge.streamer.display_name),
                    message: reason
                        .clone()
                        .unwrap_or_else(|| "Viewer activity rose unusually quickly.".to_owned()),
                    reason: reason.unwrap_or_else(|| "Unusual viewer acceleration".to_owned()),
                    confidence: 0.85,
                };
                if let Err(error) = service.maybe_notify(candidate).await {
                    tracing::warn!(
                        target: LOG,
                        "Viewer surge notification failed for {}: {error}",
                        surge.streamer.display_name
                    );
                }
            });
        }

        let start_listener = {
            let mut listeners = self.state();
            if Self::is_summary_target(&observation, &state) {
                listeners.summary_targets.insert(id.clone());
                listeners.listening.insert(id.clone())
            } else {
                listeners.summary_targets.remove(&id);
                false
            }
        };
        if start_listener {
            self.start_listener(id);
        }
        Ok(())
    }

    async fn reset_stages(
        &self,
        observation: &LiveObservation,
        state: &LivestreamIntelligenceData,
        now: i64,
    ) {
        let id = &observation.streamer.id;
        let session = observation.session_started_at();
        let mut metadata = stage(PipelineStatus::Skipped);
        metadata.eligible = Some(false);
        metadata.finished_at = Some(now);
        metadata.detail = Some("Title-only LLM classification is disabled".to_owned());
        self.set_stage(id, session, PipelineStage::Metadata, metadata)
            .await;

        let voice_target = self.is_voice_target(observation);
        let mut voice = stage(PipelineStatus::Idle);
        voice.eligible = Some(voice_target);
        voice.detail = Some(
            if voice_target {
                "Eligible DGG third-party stream"
            } else if is_destiny_owned_stream(&observation.streamer) {
                "Destiny's own stream is always excluded"
            } else {
                "Not a DGG third-party voice target"
            }
            .to_owned(),
        );
        self.set_stage(id, session, PipelineStage::Voice, voice)
            .await;

        let summary_target = Self::is_summary_target(observation, state);
        let mut summary = stage(if state.summary.is_some() {
            PipelineStatus::Success
        } else {
            PipelineStatus::Idle
        });
        summary.eligible = Some(summary_target);
        summary.finished_at = state.summary.as_ref().map(|s| s.updated_at);
        summary.detail = Some(match &state.summary {
            Some(s) => s.topic.clone(),
            None if summary_target => "Eligible for rolling summaries".to_owned(),
            None => "Waiting for relevance, importance, or anomaly gate".to_owned(),
        });
        summary.metrics = state.summary.as_ref().map(|s| {
            metrics([
                ("confidence", metric_number(s.confidence)),
                ("audioSeconds", metric_number(s.window_seconds)),
            ])
        });
        self.set_stage(id, session, PipelineStage::Summary, summary)
            .await;

        if let Some(alert) = &state.latest_alert {
            let mut value = stage(PipelineStatus::Success);
            value.eligible = Some(true);
            value.finished_at = Some(alert.created_at);
            value.detail = Some(alert.title.clone());
            value.metrics = Some(metrics([
                ("type", metric_string(alert.alert_type.as_str())),
                ("confidence", metric_number(alert.confidence)),
            ]));
            self.set_stage(id, session, PipelineStage::Alert, value)
                .await;
        }
    }

    /// The streamer went offline: end the session and forget in-memory state.
    pub async fn observe_offline(&self, streamer_id: &str) -> Result<(), ServiceError> {
        let store = &self.deps().store;
        let current = get_intelligence(store, streamer_id).await?;
        let diagnostics = get_diagnostics(store, streamer_id).await?;
        let finished_at = self.now();
        if let Some(current) = current.filter(|_| self.is_active(streamer_id)) {
            self.record(NewLivestreamEvent::new(
                streamer_id,
                Some(current.session_started_at),
                LivestreamEventKind::Session,
                EventStatus::Info,
                "Live session ended",
            ))
            .await;
            for (name, value) in diagnostics.map(|d| d.stages).unwrap_or_default() {
                let Some(pipeline_stage) = PipelineStage::parse(&name) else {
                    continue;
                };
                if value.status != PipelineStatus::Running {
                    continue;
                }
                let duration_ms = value
                    .started_at
                    .filter(|started| *started != 0)
                    .map(|started| finished_at - started);
                let next = StageDiagnostic {
                    status: PipelineStatus::Skipped,
                    finished_at: Some(finished_at),
                    duration_ms,
                    detail: Some("Stream ended before this operation completed".to_owned()),
                    ..value
                };
                self.set_stage(
                    streamer_id,
                    current.session_started_at,
                    pipeline_stage,
                    next,
                )
                .await;
            }
        }
        let mut state = self.state();
        state.active.shift_remove(streamer_id);
        state.anomaly.clear(streamer_id);
        state.typical_peaks.remove(streamer_id);
        state.voice_evidence.clear(streamer_id);
        state.pending_voice.remove(streamer_id);
        state.last_voice_sample.remove(streamer_id);
        state.summary_targets.remove(streamer_id);
        Ok(())
    }

    /// After every live-check tick: schedule voice samples for the best targets.
    pub async fn after_tick(&self) -> Result<(), ServiceError> {
        let now = self.now();
        let candidates: Vec<LiveObservation> = self
            .state()
            .active
            .values()
            .filter(|o| self.is_voice_target(o))
            .cloned()
            .collect();
        for observation in select_voice_targets(candidates, self.settings().max_voice_targets) {
            if self.voice_due(&observation.streamer.id, now) {
                self.schedule_voice_sample(observation).await;
            }
        }
        Ok(())
    }

    /// Drains in dependency order: listeners stop capturing and finish their
    /// last window, capture jobs may enqueue speech work, which may enqueue
    /// classifier work.
    pub async fn close(&self) {
        self.inner.shutdown.cancel();
        self.inner.listen_queue.close().await;
        self.inner.capture_queue.close().await;
        self.inner.speech_queue.close().await;
        self.inner.summary_speech_queue.close().await;
        self.inner.llm_queue.close().await;
        self.inner.background_queue.close().await;
    }

    async fn schedule_voice_sample(&self, observation: LiveObservation) {
        let id = observation.streamer.id.clone();
        let session = observation.session_started_at();
        let started_at = self.now();
        {
            let mut state = self.state();
            state.pending_voice.insert(id.clone(), session);
            state.last_voice_sample.insert(id.clone(), started_at);
        }
        let mut running = stage(PipelineStatus::Running);
        running.eligible = Some(true);
        running.started_at = Some(started_at);
        running.detail = Some("Capturing a bounded voice sample".to_owned());
        self.set_stage(&id, session, PipelineStage::Voice, running)
            .await;
        let service = self.clone();
        self.inner.capture_queue.fork(async move {
            let _pending = PendingGuard {
                inner: Arc::clone(&service.inner),
                streamer_id: id.clone(),
                session_started_at: session,
            };
            if let Err(error) = Box::pin(service.voice_job(&observation, started_at)).await {
                let finished_at = service.now();
                let detail = failure_detail(&error);
                let not_live = is_not_live(&error);
                let mut failed = stage(if not_live {
                    PipelineStatus::Skipped
                } else {
                    PipelineStatus::Error
                });
                failed.eligible = Some(true);
                failed.started_at = Some(started_at);
                failed.finished_at = Some(finished_at);
                failed.next_at = Some(started_at + service.settings().voice_interval_ms());
                failed.duration_ms = Some(finished_at - started_at);
                failed.detail = Some(detail.clone());
                service
                    .set_stage(&id, session, PipelineStage::Voice, failed)
                    .await;
                if not_live {
                    tracing::debug!(
                        target: LOG,
                        "Voice sampling skipped for {}: {detail}",
                        observation.streamer.display_name
                    );
                    return;
                }
                service
                    .record(
                        NewLivestreamEvent::new(
                            &id,
                            Some(session),
                            LivestreamEventKind::Voice,
                            EventStatus::Error,
                            "Voice scan failed",
                        )
                        .detail(detail.clone())
                        .duration_ms(finished_at - started_at),
                    )
                    .await;
                tracing::warn!(
                    target: LOG,
                    "Voice sampling failed for {}: {detail}",
                    observation.streamer.display_name
                );
            }
        });
    }

    async fn voice_job(
        &self,
        observation: &LiveObservation,
        started_at: i64,
    ) -> Result<(), ServiceError> {
        let audio = self
            .deps()
            .capture
            .capture(
                &observation.stream_url(),
                self.settings().voice_sample_seconds,
            )
            .await?;
        let samples = Arc::new(audio.samples);
        self.inner
            .speech_queue
            .run(async {
                let matched = self
                    .deps()
                    .speech
                    .detect_destiny(Arc::clone(&samples))
                    .await?;
                Box::pin(self.handle_voice_match(observation, &samples, matched, started_at)).await
            })
            .await?
    }

    async fn handle_voice_match(
        &self,
        observation: &LiveObservation,
        samples: &Arc<Vec<f32>>,
        matched: SpeakerMatch,
        started_at: i64,
    ) -> Result<(), ServiceError> {
        let id = &observation.streamer.id;
        let now = self.now();
        let Some(current) = self.current_session(observation).await? else {
            return Ok(());
        };
        let evidence = self.state().voice_evidence.observe(
            id,
            matched.matched_windows,
            matched.checked_windows,
            now,
        );
        let prior =
            get_latest_destiny_confirmation(&self.deps().store, id, current.session_started_at)
                .await?;
        let confirmed_presence = match (&current.destiny_presence, prior) {
            (Some(presence), _) if presence.state == PresenceState::Confirmed => {
                Some(presence.clone())
            }
            (_, Some(prior)) => {
                let metric =
                    |name: &str| js_number(prior.metrics.as_ref().and_then(|m| m.get(name)));
                Some(DestinyPresence {
                    state: PresenceState::Confirmed,
                    confidence: js_min(metric("speakerConfidence"), metric("assessmentConfidence")),
                    detected_at: prior.created_at,
                    reason: prior
                        .detail
                        .clone()
                        .unwrap_or_else(|| "Live conversation previously confirmed".to_owned()),
                    extra: Extra::new(),
                })
            }
            (presence, None) => presence.clone(),
        };
        let action = decide_voice_match_action(evidence, confirmed_presence.as_ref());
        let voice_detail = format!(
            "{}/{} windows matched at {}% confidence",
            matched.matched_windows,
            matched.checked_windows,
            percent(matched.confidence)
        );
        let mut done = stage(PipelineStatus::Success);
        done.eligible = Some(true);
        done.started_at = Some(started_at);
        done.finished_at = Some(now);
        done.next_at = Some(started_at + self.settings().voice_interval_ms());
        done.duration_ms = Some(now - started_at);
        done.detail = Some(if evidence == VoiceEvidenceDecision::None {
            format!("No Destiny evidence; {voice_detail}")
        } else {
            voice_detail.clone()
        });
        done.metrics = Some(metrics([
            ("confidence", metric_number(matched.confidence)),
            (
                "matchedWindows",
                metric_number(f64::from(matched.matched_windows)),
            ),
            (
                "checkedWindows",
                metric_number(f64::from(matched.checked_windows)),
            ),
            (
                "evidence",
                metric_string(if action == VoiceMatchAction::RetainConfirmed {
                    "confirmed"
                } else {
                    evidence.as_str()
                }),
            ),
        ]));
        self.set_stage(id, current.session_started_at, PipelineStage::Voice, done)
            .await;

        match action {
            VoiceMatchAction::Ignore => Ok(()),
            VoiceMatchAction::RetainConfirmed => {
                let Some(confirmed) = confirmed_presence else {
                    return Ok(());
                };
                let presence = DestinyPresence {
                    detected_at: now,
                    ..confirmed
                };
                let Some(mut latest) = self.current_session(observation).await? else {
                    return Ok(());
                };
                latest.destiny_presence = Some(presence.clone());
                latest.updated_at = now;
                save_intelligence(&self.deps().store, latest.clone()).await?;
                if !alert_sent_in_session(&latest, LivestreamAlertType::DestinyGuest) {
                    self.maybe_notify(AlertCandidate {
                        observation,
                        alert_type: LivestreamAlertType::DestinyGuest,
                        title: format!("Destiny is on {}", observation.streamer.display_name),
                        message: presence.reason.clone(),
                        reason: "Previously confirmed live participation plus fresh voice evidence"
                            .to_owned(),
                        confidence: presence.confidence,
                    })
                    .await?;
                }
                Ok(())
            }
            VoiceMatchAction::RecordPossible => {
                let Some(mut latest) = self.current_session(observation).await? else {
                    return Ok(());
                };
                latest.destiny_presence = Some(DestinyPresence {
                    state: PresenceState::Possible,
                    confidence: matched.confidence,
                    detected_at: now,
                    reason: format!(
                        "{}/{} voice windows matched; awaiting confirmation",
                        matched.matched_windows, matched.checked_windows
                    ),
                    extra: Extra::new(),
                });
                latest.updated_at = now;
                save_intelligence(&self.deps().store, latest).await?;
                self.record(
                    NewLivestreamEvent::new(
                        id,
                        Some(current.session_started_at),
                        LivestreamEventKind::Voice,
                        EventStatus::Warning,
                        "Possible Destiny voice match",
                    )
                    .detail(format!(
                        "{voice_detail}; awaiting another independent sample"
                    ))
                    .duration_ms(now - started_at)
                    .metrics(metrics([
                        ("confidence", metric_number(matched.confidence)),
                        (
                            "matchedWindows",
                            metric_number(f64::from(matched.matched_windows)),
                        ),
                        (
                            "checkedWindows",
                            metric_number(f64::from(matched.checked_windows)),
                        ),
                    ])),
                )
                .await;
                Ok(())
            }
            VoiceMatchAction::Verify => {
                self.verify_destiny(observation, &current, samples, matched)
                    .await
            }
        }
    }

    async fn verify_destiny(
        &self,
        observation: &LiveObservation,
        current: &LivestreamIntelligenceData,
        samples: &Arc<Vec<f32>>,
        matched: SpeakerMatch,
    ) -> Result<(), ServiceError> {
        let id = &observation.streamer.id;
        let context = self
            .deps()
            .capture
            .capture(&observation.stream_url(), VERIFY_CONTEXT_SECONDS)
            .await?;
        let mut combined = Vec::with_capacity(samples.len() + context.samples.len());
        combined.extend_from_slice(samples);
        combined.extend_from_slice(&context.samples);
        #[allow(clippy::cast_precision_loss)]
        let combined_seconds = combined.len() as f64 / f64::from(SAMPLE_RATE);
        let transcript = self.deps().speech.transcribe(Arc::new(combined)).await?;
        self.record_local_transcription("verify-destiny", combined_seconds)
            .await;
        if omni_core::js::utf16_len(&transcript) < 30 {
            self.record(
                NewLivestreamEvent::new(
                    id,
                    Some(current.session_started_at),
                    LivestreamEventKind::Voice,
                    EventStatus::Warning,
                    "Voice confirmation skipped",
                )
                .detail("The confirmation window did not contain enough transcribed speech"),
            )
            .await;
            return Ok(());
        }
        let classifier = Arc::clone(&self.deps().classifier);
        let assessment = self
            .inner
            .llm_queue
            .run(classifier.assess(TranscriptAssessmentInput {
                display_name: observation.streamer.display_name.clone(),
                title: observation.status.primary_title.clone(),
                transcript,
                speakers: None,
                window_seconds: combined_seconds,
                previous_summary: current.summary.as_ref().map(|s| s.text.clone()),
                previous_topic: current.summary.as_ref().map(|s| s.topic.clone()),
                viewer_anomaly: current.trend.as_ref().and_then(|t| t.reason.clone()),
                speaker_match_confidence: Some(matched.confidence),
                testing_destiny_presence: true,
            }))
            .await??;
        let assessment = match assessment {
            Some(a) if a.destiny_is_live_participant && a.confidence >= 0.65 => a,
            other => {
                return self
                    .record_unconfirmed(id, current.session_started_at, other.as_ref())
                    .await;
            }
        };
        let Some(mut latest) = self.current_session(observation).await? else {
            return Ok(());
        };
        let confirmed_at = self.now();
        let confidence = js_min(matched.confidence, assessment.confidence);
        latest.destiny_presence = Some(DestinyPresence {
            state: PresenceState::Confirmed,
            confidence,
            detected_at: confirmed_at,
            reason: assessment.summary.clone(),
            extra: Extra::new(),
        });
        latest.updated_at = confirmed_at;
        let session = latest.session_started_at;
        save_intelligence(&self.deps().store, latest).await?;
        self.record(
            NewLivestreamEvent::new(
                id,
                Some(session),
                LivestreamEventKind::Voice,
                EventStatus::Success,
                DESTINY_CONFIRMED_EVENT_TITLE,
            )
            .detail(assessment.summary.clone())
            .metrics(metrics([
                ("speakerConfidence", metric_number(matched.confidence)),
                ("assessmentConfidence", metric_number(assessment.confidence)),
            ])),
        )
        .await;
        self.maybe_notify(AlertCandidate {
            observation,
            alert_type: LivestreamAlertType::DestinyGuest,
            title: format!("Destiny is on {}", observation.streamer.display_name),
            message: assessment.summary.clone(),
            reason: format!(
                "{} repeated voice matches plus live-conversation context",
                matched.matched_windows
            ),
            confidence,
        })
        .await
    }

    async fn record_unconfirmed(
        &self,
        id: &str,
        session: i64,
        assessment: Option<&TranscriptAssessment>,
    ) -> Result<(), ServiceError> {
        self.record(
            NewLivestreamEvent::new(
                id,
                Some(session),
                LivestreamEventKind::Voice,
                EventStatus::Info,
                "Destiny live participation not confirmed",
            )
            .detail(assessment.map_or_else(
                || "Transcript assessment skipped by the budget gate".to_owned(),
                |a| a.summary.clone(),
            ))
            .metrics(metrics([(
                "assessmentConfidence",
                metric_nullable(assessment.map(|a| a.confidence)),
            )])),
        )
        .await;
        Ok(())
    }

    fn start_listener(&self, streamer_id: String) {
        let service = self.clone();
        self.inner.listen_queue.fork(async move {
            let _guard = ListenerGuard {
                inner: Arc::clone(&service.inner),
                streamer_id: streamer_id.clone(),
            };
            service.listen(&streamer_id).await;
        });
    }

    /// The latest observation of a streamer that is still a live summary target.
    fn listen_target(&self, streamer_id: &str) -> Option<LiveObservation> {
        if self.inner.shutdown.is_cancelled() {
            return None;
        }
        let state = self.state();
        if !state.summary_targets.contains(streamer_id) {
            return None;
        }
        state.active.get(streamer_id).cloned()
    }

    /// Captures back-to-back windows while the streamer stays a live summary
    /// target, summarizing each window while the next one records.
    async fn listen(&self, streamer_id: &str) {
        let shutdown = self.inner.shutdown.clone();
        let interval = self
            .settings()
            .summary_interval_seconds
            .max(MIN_LISTEN_WINDOW_SECONDS);
        let mut window_seconds = FIRST_WINDOW_SECONDS.min(interval);
        let mut pending: Option<SummaryWindow> = None;
        while let Some(observation) = self.listen_target(streamer_id) {
            let started_at = self.now();
            let session = observation.session_started_at();
            // With a window pending, its summary result is the stage to show.
            if pending.is_none() {
                let mut running = stage(PipelineStatus::Running);
                running.eligible = Some(true);
                running.started_at = Some(started_at);
                running.next_at = Some(started_at + i64::from(window_seconds) * 1000);
                running.detail = Some("Listening to the current window".to_owned());
                self.set_stage(streamer_id, session, PipelineStage::Summary, running)
                    .await;
            }
            let url = observation.stream_url();
            let capture = async {
                tokio::select! {
                    result = self.deps().capture.capture(&url, window_seconds) => Some(result),
                    () = shutdown.cancelled() => None,
                }
            };
            let previous = pending.take();
            let summarize = async {
                if let Some(window) = previous {
                    self.summarize(window).await;
                }
            };
            let (captured, ()) = tokio::join!(capture, summarize);
            let Some(captured) = captured else {
                break;
            };
            window_seconds = interval;
            let retry_after = match captured {
                Ok(audio) if audio.duration_seconds >= MIN_WINDOW_SECONDS => {
                    pending = Some(SummaryWindow {
                        observation,
                        started_at,
                        duration_seconds: audio.duration_seconds,
                        samples: Arc::new(audio.samples),
                    });
                    None
                }
                Ok(_) => {
                    let finished_at = self.now();
                    self.set_stage(
                        streamer_id,
                        session,
                        PipelineStage::Summary,
                        self.finished_stage(
                            PipelineStatus::Skipped,
                            started_at,
                            finished_at,
                            "The captured window was too short to summarize",
                        ),
                    )
                    .await;
                    Some(LISTEN_RETRY_DELAY)
                }
                Err(error) => Some(
                    self.capture_failed(&observation, started_at, &ServiceError::Audio(error))
                        .await,
                ),
            };
            if let Some(delay) = retry_after {
                tokio::select! {
                    () = tokio::time::sleep(delay) => {}
                    () = shutdown.cancelled() => break,
                }
            }
        }
        // At shutdown the last window is dropped: summarizing it would outlast
        // the drain window.
        if let Some(window) = pending.filter(|_| !shutdown.is_cancelled()) {
            self.summarize(window).await;
        }
    }

    /// Records a failed window capture; returns how long to wait before
    /// listening again.
    async fn capture_failed(
        &self,
        observation: &LiveObservation,
        started_at: i64,
        error: &ServiceError,
    ) -> Duration {
        let not_live = is_not_live(error);
        self.summary_failed(observation, started_at, error).await;
        if not_live {
            Duration::from_secs(u64::from(self.settings().summary_interval_seconds))
        } else {
            LISTEN_RETRY_DELAY
        }
    }

    async fn summary_failed(
        &self,
        observation: &LiveObservation,
        started_at: i64,
        error: &ServiceError,
    ) {
        let id = &observation.streamer.id;
        let session = observation.session_started_at();
        let finished_at = self.now();
        let detail = failure_detail(error);
        let not_live = is_not_live(error);
        let failed = self.finished_stage(
            if not_live {
                PipelineStatus::Skipped
            } else {
                PipelineStatus::Error
            },
            started_at,
            finished_at,
            detail.clone(),
        );
        self.set_stage(id, session, PipelineStage::Summary, failed)
            .await;
        if not_live {
            tracing::debug!(
                target: LOG,
                "Rolling summary skipped for {}: {detail}",
                observation.streamer.display_name
            );
            return;
        }
        self.record(
            NewLivestreamEvent::new(
                id,
                Some(session),
                LivestreamEventKind::Summary,
                EventStatus::Error,
                "Rolling summary failed",
            )
            .detail(detail.clone())
            .duration_ms(finished_at - started_at),
        )
        .await;
        tracing::warn!(
            target: LOG,
            "Rolling summary failed for {}: {detail}",
            observation.streamer.display_name
        );
    }

    fn finished_stage(
        &self,
        status: PipelineStatus,
        started_at: i64,
        finished_at: i64,
        detail: impl Into<String>,
    ) -> StageDiagnostic {
        let mut value = stage(status);
        value.eligible = Some(true);
        value.started_at = Some(started_at);
        value.finished_at = Some(finished_at);
        value.next_at = Some(finished_at + self.settings().summary_interval_ms());
        value.duration_ms = Some(finished_at - started_at);
        value.detail = Some(detail.into());
        value
    }

    async fn summarize(&self, window: SummaryWindow) {
        if let Err(error) = self.summary_job(&window).await {
            self.summary_failed(&window.observation, window.started_at, &error)
                .await;
        }
    }

    /// Diarizes the window and transcribes each speaker turn. A turn that
    /// fails to transcribe is skipped; the window fails only if every turn does.
    async fn attributed_transcript(
        &self,
        samples: Arc<Vec<f32>>,
    ) -> Result<AttributedTranscript, ServiceError> {
        let speech = &self.deps().speech;
        let queue = &self.inner.summary_speech_queue;
        let attribution = queue
            .run(speech.attribute_speakers(Arc::clone(&samples)))
            .await??;
        let mut turns = Vec::with_capacity(attribution.turns.len());
        let mut last_error = None;
        for turn in &attribution.turns {
            let Some(audio) = samples.get(turn.start..turn.end) else {
                continue;
            };
            match queue
                .run(speech.transcribe(Arc::new(audio.to_vec())))
                .await?
            {
                Ok(text) => turns.push(TranscribedTurn {
                    speaker: turn.speaker,
                    text,
                }),
                Err(error) => {
                    tracing::debug!(target: LOG, "Skipping an untranscribable speaker turn: {error}");
                    last_error = Some(error);
                }
            }
        }
        if let Some(error) = last_error.filter(|_| turns.is_empty()) {
            return Err(error.into());
        }
        Ok(AttributedTranscript {
            text: format_attributed(&turns, &attribution.speakers),
            speakers: describe_speakers(&attribution.speakers),
            speaker_count: attribution.speakers.len(),
            spoken_chars: turns
                .iter()
                .map(|t| omni_core::js::utf16_len(&t.text))
                .sum(),
        })
    }

    async fn summary_job(&self, window: &SummaryWindow) -> Result<(), ServiceError> {
        let observation = &window.observation;
        let id = &observation.streamer.id;
        let session = observation.session_started_at();
        let started_at = window.started_at;
        let processing_started = self.now();
        let duration_seconds = window.duration_seconds;
        let AttributedTranscript {
            text: transcript,
            speakers,
            speaker_count,
            spoken_chars,
        } = self
            .attributed_transcript(Arc::clone(&window.samples))
            .await?;
        #[allow(clippy::cast_precision_loss)]
        let speech_seconds = (self.now() - processing_started) as f64 / 1000.0;
        self.record_local_transcription("rolling-summary", duration_seconds)
            .await;
        if spoken_chars < 30 {
            let finished_at = self.now();
            self.set_stage(
                id,
                session,
                PipelineStage::Summary,
                self.finished_stage(
                    PipelineStatus::Skipped,
                    started_at,
                    finished_at,
                    "Not enough speech in the captured window",
                ),
            )
            .await;
            self.record(
                NewLivestreamEvent::new(
                    id,
                    Some(session),
                    LivestreamEventKind::Summary,
                    EventStatus::Info,
                    "Summary window skipped",
                )
                .detail("Not enough transcribed speech")
                .duration_ms(finished_at - started_at),
            )
            .await;
            return Ok(());
        }
        let Some(current) = self.current_session(observation).await? else {
            return Ok(());
        };
        let classifier = Arc::clone(&self.deps().classifier);
        let input = TranscriptAssessmentInput {
            display_name: observation.streamer.display_name.clone(),
            title: observation.status.primary_title.clone(),
            transcript: transcript.clone(),
            speakers: Some(speakers.clone()),
            window_seconds: duration_seconds,
            previous_summary: current.summary.as_ref().map(|s| s.text.clone()),
            previous_topic: current.summary.as_ref().map(|s| s.topic.clone()),
            viewer_anomaly: current.trend.as_ref().and_then(|t| t.reason.clone()),
            speaker_match_confidence: None,
            testing_destiny_presence: false,
        };
        // Spend is measured inside the LLM queue, which serializes model calls,
        // so concurrent windows do not count each other's cost.
        let (store, tz) = (&self.deps().store, &self.settings().tz);
        let (assessment, cost_cents) = self
            .inner
            .llm_queue
            .run(async {
                let before = livestream_spend_cents(store, self.now(), tz).await?;
                let assessment = classifier.assess(input).await?;
                let after = livestream_spend_cents(store, self.now(), tz).await?;
                Ok::<_, ServiceError>((assessment, (after - before).max(0.0)))
            })
            .await??;
        let Some(assessment) = assessment else {
            let finished_at = self.now();
            self.set_stage(
                id,
                session,
                PipelineStage::Summary,
                self.finished_stage(
                    PipelineStatus::Skipped,
                    started_at,
                    finished_at,
                    "Monthly budget could not cover transcript assessment",
                ),
            )
            .await;
            self.record(
                NewLivestreamEvent::new(
                    id,
                    Some(session),
                    LivestreamEventKind::Summary,
                    EventStatus::Warning,
                    "Summary assessment skipped",
                )
                .detail("Monthly intelligence budget gate"),
            )
            .await;
            return Ok(());
        };
        self.save_assessment(window, &transcript, &speakers, &assessment)
            .await?;
        let finished_at = self.now();
        let audio_seconds = math_round(duration_seconds);
        let mut done = self.finished_stage(
            PipelineStatus::Success,
            started_at,
            finished_at,
            assessment.topic.clone(),
        );
        #[allow(clippy::cast_precision_loss)]
        let transcript_characters = omni_core::js::utf16_len(&transcript) as f64;
        #[allow(clippy::cast_precision_loss)]
        let speaker_count = speaker_count as f64;
        done.metrics = Some(metrics([
            ("confidence", metric_number(assessment.confidence)),
            ("audioSeconds", metric_number(audio_seconds)),
            ("speechSeconds", metric_number(math_round(speech_seconds))),
            ("speakers", metric_number(speaker_count)),
            ("transcriptCharacters", metric_number(transcript_characters)),
            ("costCents", metric_number(cost_cents)),
        ]));
        self.set_stage(id, session, PipelineStage::Summary, done)
            .await;
        self.record(
            NewLivestreamEvent::new(
                id,
                Some(session),
                LivestreamEventKind::Summary,
                EventStatus::Success,
                "Now summary updated",
            )
            .detail(assessment.topic.clone())
            .duration_ms(finished_at - started_at)
            .cost_cents(cost_cents)
            .metrics(metrics([
                ("confidence", metric_number(assessment.confidence)),
                ("audioSeconds", metric_number(audio_seconds)),
                ("speakers", metric_number(speaker_count)),
            ])),
        )
        .await;
        Ok(())
    }

    async fn save_assessment(
        &self,
        window: &SummaryWindow,
        transcript: &str,
        speakers: &str,
        assessment: &TranscriptAssessment,
    ) -> Result<(), ServiceError> {
        let observation = &window.observation;
        let Some(mut current) = self.current_session(observation).await? else {
            return Ok(());
        };
        let now = self.now();
        let transcript_len = omni_core::js::utf16_len(transcript);
        current.summary = Some(RollingSummary {
            text: assessment.summary.clone(),
            topic: assessment.topic.clone(),
            confidence: assessment.confidence,
            transcript_excerpt: omni_core::js::utf16_slice(
                transcript,
                transcript_len.saturating_sub(TRANSCRIPT_EXCERPT_CHARS),
                transcript_len,
            )
            .into_owned(),
            updated_at: now,
            window_seconds: math_round(window.duration_seconds),
            speakers: Some(speakers.to_owned()),
            extra: Extra::new(),
        });
        let same_topic = current
            .chapters
            .last()
            .is_some_and(|last| are_same_livestream_topic(&last.title, &assessment.topic));
        if same_topic {
            if let Some(last) = current.chapters.last_mut() {
                last.summary = assessment.summary.clone();
            }
        } else {
            #[allow(clippy::cast_precision_loss)]
            let started_at = window.started_at as f64;
            current.chapters.push(LivestreamChapter {
                chapter_id: omni_core::ids::uuid_v4(),
                started_at,
                title: assessment.topic.clone(),
                summary: assessment.summary.clone(),
                extra: Extra::new(),
            });
            let excess = current.chapters.len().saturating_sub(MAX_CHAPTERS);
            current.chapters.drain(..excess);
        }
        current.updated_at = now;
        save_intelligence(&self.deps().store, current).await?;
        if let (Some(alert_type), Some(alert_reason)) =
            (assessment.alert_type, assessment.alert_reason.as_ref())
            && !alert_reason.is_empty()
            && assessment.importance >= 80.0
            && assessment.confidence >= 0.75
        {
            let result = self
                .maybe_notify(AlertCandidate {
                    observation,
                    alert_type: alert_type.into(),
                    title: format!(
                        "{}: {}",
                        observation.streamer.display_name, assessment.topic
                    ),
                    message: assessment.summary.clone(),
                    reason: alert_reason.clone(),
                    confidence: assessment.confidence,
                })
                .await;
            if let Err(error) = result {
                tracing::warn!(
                    target: LOG,
                    "Semantic alert failed for {}: {error}",
                    observation.streamer.display_name
                );
            }
        }
        Ok(())
    }

    async fn record_local_transcription(&self, operation: &str, seconds: f64) {
        self.deps()
            .costs
            .record(NewCostEvent {
                category: CostCategory::Transcription,
                feature: crate::classifier::FEATURE.to_owned(),
                operation: operation.to_owned(),
                service: "self-hosted".to_owned(),
                model: Some(TRANSCRIPTION_MODEL.to_owned()),
                cost_cents: Some(0.0),
                price_status: CostPriceStatus::Free,
                usage: CostUsage {
                    requests: Some(1.0),
                    characters: Some(math_round(seconds)),
                    ..CostUsage::default()
                },
                event_id: None,
                incurred_at: None,
                run_id: None,
            })
            .await;
    }

    async fn maybe_notify(&self, candidate: AlertCandidate<'_>) -> Result<(), ServiceError> {
        let observation = candidate.observation;
        let id = &observation.streamer.id;
        let Some(current) = self.current_session(observation).await? else {
            return Ok(());
        };
        let session = current.session_started_at;
        let now = self.now();
        let skipped = |detail: String| {
            let mut value = stage(PipelineStatus::Skipped);
            value.eligible = Some(true);
            value.finished_at = Some(now);
            value.detail = Some(detail);
            value.metrics = Some(metrics([
                ("type", metric_string(candidate.alert_type.as_str())),
                ("confidence", metric_number(candidate.confidence)),
            ]));
            value
        };
        let digest = build_feedback_digest(&self.deps().store, 100).await?;
        let floor = livestream_alert_confidence_floor(
            candidate.alert_type,
            &digest,
            self.settings().destiny_speaker_threshold,
        );
        if candidate.confidence < floor {
            let detail = format!(
                "{}% confidence was below the {}% alert threshold",
                percent(candidate.confidence),
                percent(floor)
            );
            self.set_stage(id, session, PipelineStage::Alert, skipped(detail))
                .await;
            return Ok(());
        }
        let key = format!("{id}:{session}:{}", candidate.alert_type.as_str());
        let previous = self.state().alert_times.get(&key).copied().unwrap_or(0);
        if now - previous < ALERT_COOLDOWN_MS {
            self.set_stage(
                id,
                session,
                PipelineStage::Alert,
                skipped("A matching alert was already sent within the 30-minute cooldown".into()),
            )
            .await;
            return Ok(());
        }
        let once_per_session = matches!(
            candidate.alert_type,
            LivestreamAlertType::DestinyGuest | LivestreamAlertType::ViewerSurge
        );
        if once_per_session && alert_sent_in_session(&current, candidate.alert_type) {
            let detail = if candidate.alert_type == LivestreamAlertType::DestinyGuest {
                "Destiny was already reported during this live session"
            } else {
                "A viewer surge was already reported during this live session"
            };
            self.set_stage(id, session, PipelineStage::Alert, skipped(detail.into()))
                .await;
            return Ok(());
        }
        let alert = LivestreamAlertRecord {
            alert_id: omni_core::ids::uuid_v4(),
            alert_type: candidate.alert_type,
            title: candidate.title,
            message: candidate.message,
            reason: candidate.reason,
            confidence: candidate.confidence,
            created_at: now,
            extra: Extra::new(),
        };
        self.state()
            .alert_times
            .insert(key.clone(), alert.created_at);
        let alert_started_at = now;
        let mut running = stage(PipelineStatus::Running);
        running.eligible = Some(true);
        running.started_at = Some(alert_started_at);
        running.detail = Some(alert.title.clone());
        self.set_stage(id, session, PipelineStage::Alert, running)
            .await;
        let (url, url_title) = observation.status.primary.notification_url();
        let sent = self
            .deps()
            .pushover
            .send(
                PushoverChannel::Live,
                PushoverMessage {
                    message: format!("{}\n\nWhy: {}", alert.message, alert.reason),
                    title: Some(alert.title.clone()),
                    url: Some(url),
                    url_title: Some(url_title),
                    ..PushoverMessage::default()
                },
            )
            .await;
        if let Err(error) = sent {
            {
                let mut state = self.state();
                if state.alert_times.get(&key) == Some(&alert.created_at) {
                    state.alert_times.remove(&key);
                }
            }
            let finished_at = self.now();
            let detail = omni_core::js::utf16_slice(&error.body, 0, DETAIL_MAX_CHARS).into_owned();
            let mut failed = stage(PipelineStatus::Error);
            failed.eligible = Some(true);
            failed.started_at = Some(alert_started_at);
            failed.finished_at = Some(finished_at);
            failed.duration_ms = Some(finished_at - alert_started_at);
            failed.detail = Some(detail.clone());
            self.set_stage(id, session, PipelineStage::Alert, failed)
                .await;
            self.record(
                NewLivestreamEvent::new(
                    id,
                    Some(session),
                    LivestreamEventKind::Alert,
                    EventStatus::Error,
                    "Alert delivery failed",
                )
                .detail(detail),
            )
            .await;
            return Err(error.into());
        }
        let Some(mut latest) = self.current_session(observation).await? else {
            return Ok(());
        };
        let finished_at = self.now();
        let mut alerted = latest.alerted_at_by_type.take().unwrap_or_default();
        alerted.insert(alert.alert_type.as_str().to_owned(), alert.created_at);
        latest.alerted_at_by_type = Some(alerted);
        latest.latest_alert = Some(alert.clone());
        latest.updated_at = finished_at;
        let session = latest.session_started_at;
        save_intelligence(&self.deps().store, latest).await?;
        let alert_metrics = metrics([
            ("type", metric_string(alert.alert_type.as_str())),
            ("confidence", metric_number(alert.confidence)),
        ]);
        let mut done = stage(PipelineStatus::Success);
        done.eligible = Some(true);
        done.started_at = Some(alert_started_at);
        done.finished_at = Some(finished_at);
        done.duration_ms = Some(finished_at - alert_started_at);
        done.detail = Some(alert.title.clone());
        done.metrics = Some(alert_metrics.clone());
        self.set_stage(id, session, PipelineStage::Alert, done)
            .await;
        self.record(
            NewLivestreamEvent::new(
                id,
                Some(session),
                LivestreamEventKind::Alert,
                EventStatus::Success,
                format!("Alert sent: {}", alert.title),
            )
            .detail(alert.reason.clone())
            .duration_ms(finished_at - alert_started_at)
            .metrics(alert_metrics),
        )
        .await;
        Ok(())
    }
}
