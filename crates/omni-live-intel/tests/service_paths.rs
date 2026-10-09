//! Service paths the TS spec does not cover: Destiny verification through the
//! classifier, rolling summaries with chapters and semantic alerts, delivery
//! failure rollback, and the port's per-tick pull from `LiveDirectory`.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use futures::future::BoxFuture;
use omni_ai::costs::CostRecorder;
use omni_alerts::{Pushover, PushoverChannel};
use omni_core::clock::{Clock, SharedClock};
use omni_http::SideEffectMode;
use omni_live_intel::audio::{AudioError, AudioSource, CapturedAudio};
use omni_live_intel::classifier::{
    AssessmentError, TranscriptAlertType, TranscriptAssessment, TranscriptAssessmentInput,
    TranscriptClassifier,
};
use omni_live_intel::observation::{
    DggPresence, LiveObservation, LiveStatus, Platform, PlatformBinding, Streamer, StreamerTier,
};
use omni_live_intel::persistence::{get_diagnostics, get_events, get_intelligence};
use omni_live_intel::port::IntelligencePort;
use omni_live_intel::routes::IntelState;
use omni_live_intel::service::{LivestreamIntelligenceService, ServiceDeps, ServiceSettings};
use omni_live_intel::speech::{SpeakerMatch, SpeechEngine, SpeechRecognitionError};
use omni_live_intel::types::{LivestreamAlertType, PipelineStatus, PresenceState};
use omni_runtime::Ports;
use omni_runtime::ports::{LiveDirectory, LiveIntelligence as _, LiveTransition, PortError};
use omni_store::Store;
use omni_testkit::TestStore;
use serde_json::{Value, json};
use tokio_util::task::TaskTracker;

const TRANSCRIPT: &str = "Destiny: no, listen to me, I am telling you the argument is wrong here.";

struct FixedClock(AtomicI64);

impl Clock for FixedClock {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

struct Capture {
    calls: AtomicUsize,
}

impl AudioSource for Capture {
    fn capture<'a>(
        &'a self,
        _url: &'a str,
        seconds: u32,
    ) -> BoxFuture<'a, Result<CapturedAudio, AudioError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            Ok(CapturedAudio {
                samples: vec![0.0; seconds as usize * 16_000],
                sample_rate: 16_000,
                duration_seconds: f64::from(seconds),
            })
        })
    }
}

struct Speech;

impl SpeechEngine for Speech {
    fn has_voiceprint(&self) -> bool {
        true
    }
    fn detect_destiny(
        &self,
        _samples: Arc<Vec<f32>>,
    ) -> BoxFuture<'_, Result<SpeakerMatch, SpeechRecognitionError>> {
        Box::pin(async {
            Ok(SpeakerMatch {
                confidence: 0.8,
                matched_windows: 2,
                checked_windows: 4,
            })
        })
    }
    fn transcribe(
        &self,
        _samples: Arc<Vec<f32>>,
    ) -> BoxFuture<'_, Result<String, SpeechRecognitionError>> {
        Box::pin(async { Ok(TRANSCRIPT.to_owned()) })
    }
}

#[derive(Default)]
struct Classifier {
    inputs: Mutex<Vec<TranscriptAssessmentInput>>,
    reply: Mutex<Option<TranscriptAssessment>>,
}

impl TranscriptClassifier for Classifier {
    fn assess(
        &self,
        input: TranscriptAssessmentInput,
    ) -> BoxFuture<'_, Result<Option<TranscriptAssessment>, AssessmentError>> {
        self.inputs.lock().expect("lock").push(input);
        let reply = self.reply.lock().expect("lock").clone();
        Box::pin(async move { Ok(reply) })
    }
}

fn assessment(destiny: bool, alert: Option<TranscriptAlertType>) -> TranscriptAssessment {
    TranscriptAssessment {
        summary: "Destiny is arguing with the host about the claim.".into(),
        topic: "Debate over the claim".into(),
        confidence: 0.9,
        importance: 85.0,
        alert_type: alert,
        alert_reason: alert.map(|_| "A substantive debate began".into()),
        destiny_is_live_participant: destiny,
    }
}

struct Harness {
    _guard: TestStore,
    store: Store,
    clock: Arc<FixedClock>,
    pushover: Pushover,
    classifier: Arc<Classifier>,
    capture: Arc<Capture>,
}

impl Harness {
    async fn new(user: Option<&str>) -> Self {
        let clock = Arc::new(FixedClock(AtomicI64::new(1_767_225_600_000)));
        let shared: SharedClock = clock.clone();
        let guard = TestStore::new(shared).await;
        let store = guard.store.clone();
        Self {
            _guard: guard,
            store,
            clock,
            pushover: Pushover::with_credentials(
                omni_testkit::no_network(),
                user.map(str::to_owned),
                [(PushoverChannel::Live, "live-token".to_owned())],
                if user.is_some() {
                    SideEffectMode::Record
                } else {
                    SideEffectMode::Live
                },
            ),
            classifier: Arc::new(Classifier::default()),
            capture: Arc::new(Capture {
                calls: AtomicUsize::new(0),
            }),
        }
    }

    fn advance(&self, ms: i64) {
        self.clock.0.fetch_add(ms, Ordering::SeqCst);
    }

    fn service(&self) -> LivestreamIntelligenceService {
        let clock: SharedClock = self.clock.clone();
        LivestreamIntelligenceService::new(ServiceDeps {
            store: self.store.clone(),
            clock: clock.clone(),
            pushover: self.pushover.clone(),
            costs: CostRecorder::new(self.store.clone(), clock),
            capture: self.capture.clone(),
            speech: Arc::new(Speech),
            classifier: self.classifier.clone(),
            tracker: TaskTracker::new(),
            settings: ServiceSettings {
                destiny_speaker_threshold: 0.62,
                max_voice_targets: 3,
                voice_sample_seconds: 18,
                voice_sample_interval_seconds: 45,
                summary_sample_seconds: 75,
                summary_interval_seconds: 480,
                monthly_budget_usd: 3.0,
                tz: jiff::tz::TimeZone::UTC,
            },
        })
    }
}

fn guest(tier: StreamerTier) -> LiveObservation {
    LiveObservation {
        streamer: Streamer {
            id: "guest".into(),
            display_name: "Guest".into(),
            bindings: vec![PlatformBinding::new(Platform::Twitch, "guest")],
            tier,
            dgg: Some(DggPresence {
                hosted: true,
                viewers: Some(300.0),
            }),
        },
        status: LiveStatus {
            streamer_id: "guest".into(),
            primary: PlatformBinding::new(Platform::Twitch, "guest"),
            primary_title: "Talking".into(),
            started_at: 1_000,
            viewer_count: Some(500.0),
            sources: None,
        },
    }
}

async fn wait_for_event(store: &Store, title: &str) {
    for _ in 0..500 {
        let events = get_events(store, Some("guest"), 200).await.expect("events");
        if events.iter().any(|e| e.title == title) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("event {title:?} not recorded");
}

#[tokio::test]
async fn repeated_voice_evidence_is_verified_by_the_classifier_then_alerted() {
    let harness = Harness::new(Some("user")).await;
    *harness.classifier.reply.lock().expect("lock") = Some(assessment(true, None));
    let service = harness.service();
    service
        .observe_live(guest(StreamerTier::Background))
        .await
        .expect("observe");
    service.after_tick().await.expect("tick");
    wait_for_event(&harness.store, "Possible Destiny voice match").await;
    harness.advance(60_000);
    service.after_tick().await.expect("tick");
    wait_for_event(&harness.store, "Destiny confirmed as a live participant").await;
    service.close().await;

    let pushes = harness.pushover.recorded();
    assert_eq!(pushes.len(), 1);
    assert_eq!(
        pushes[0].message.title.as_deref(),
        Some("Destiny is on Guest")
    );
    assert_eq!(
        pushes[0].message.url.as_deref(),
        Some("https://www.twitch.tv/guest")
    );
    assert_eq!(
        pushes[0].message.url_title.as_deref(),
        Some("Watch on Twitch")
    );
    assert_eq!(
        pushes[0].message.message,
        "Destiny is arguing with the host about the claim.\n\nWhy: 2 repeated voice matches plus live-conversation context"
    );
    let inputs = harness.classifier.inputs.lock().expect("lock").clone();
    assert_eq!(inputs.len(), 1);
    assert!(inputs[0].testing_destiny_presence);
    assert_eq!(inputs[0].speaker_match_confidence, Some(0.8));
    // Voice sample (18 s), then verification context (45 s), then the second sample.
    assert_eq!(harness.capture.calls.load(Ordering::SeqCst), 3);
    let state = get_intelligence(&harness.store, "guest")
        .await
        .expect("read")
        .expect("present");
    let presence = state.destiny_presence.expect("presence");
    assert_eq!(presence.state, PresenceState::Confirmed);
    assert_eq!(presence.confidence, 0.8);
    assert_eq!(
        state.latest_alert.map(|a| a.alert_type),
        Some(LivestreamAlertType::DestinyGuest)
    );
}

#[tokio::test]
async fn summaries_build_chapters_and_send_semantic_alerts() {
    let harness = Harness::new(Some("user")).await;
    *harness.classifier.reply.lock().expect("lock") =
        Some(assessment(false, Some(TranscriptAlertType::Debate)));
    let service = harness.service();
    service
        .observe_live(guest(StreamerTier::Primary))
        .await
        .expect("observe");
    wait_for_event(&harness.store, "Now summary updated").await;
    service.close().await;

    let state = get_intelligence(&harness.store, "guest")
        .await
        .expect("read")
        .expect("present");
    let summary = state.summary.expect("summary");
    assert_eq!(summary.topic, "Debate over the claim");
    assert_eq!(summary.window_seconds, 75.0);
    assert_eq!(summary.transcript_excerpt, TRANSCRIPT);
    assert_eq!(state.chapters.len(), 1);
    assert_eq!(state.chapters[0].title, "Debate over the claim");
    let pushes = harness.pushover.recorded();
    assert_eq!(pushes.len(), 1);
    assert_eq!(
        pushes[0].message.title.as_deref(),
        Some("Guest: Debate over the claim")
    );
    let diagnostics = get_diagnostics(&harness.store, "guest")
        .await
        .expect("read")
        .expect("present");
    let stage = &diagnostics.stages["summary"];
    assert_eq!(stage.status, PipelineStatus::Success);
    assert_eq!(stage.next_at, stage.started_at.map(|s| s + 480_000));
    let inputs = harness.classifier.inputs.lock().expect("lock").clone();
    assert!(!inputs[0].testing_destiny_presence);
    assert_eq!(inputs[0].speaker_match_confidence, None);
}

#[tokio::test]
async fn a_failed_delivery_rolls_back_the_cooldown_and_records_the_error() {
    // Live mode against a refuse-all client: the Pushover request fails.
    let harness = Harness::new(Some("user")).await;
    let harness = Harness {
        pushover: Pushover::with_credentials(
            omni_testkit::no_network(),
            Some("user".into()),
            [(PushoverChannel::Live, "live-token".to_owned())],
            SideEffectMode::Live,
        ),
        ..harness
    };
    *harness.classifier.reply.lock().expect("lock") =
        Some(assessment(false, Some(TranscriptAlertType::Debate)));
    let service = harness.service();
    service
        .observe_live(guest(StreamerTier::Primary))
        .await
        .expect("observe");
    wait_for_event(&harness.store, "Alert delivery failed").await;
    service.close().await;
    let diagnostics = get_diagnostics(&harness.store, "guest")
        .await
        .expect("read")
        .expect("present");
    assert_eq!(diagnostics.stages["alert"].status, PipelineStatus::Error);
    let state = get_intelligence(&harness.store, "guest")
        .await
        .expect("read")
        .expect("present");
    assert!(state.latest_alert.is_none());
    assert!(state.alerted_at_by_type.is_none());
}

struct Directory {
    statuses: Mutex<Vec<Value>>,
}

impl LiveDirectory for Directory {
    fn streamers(&self) -> BoxFuture<'_, Result<Vec<Value>, PortError>> {
        Box::pin(async {
            Ok(vec![
                // `LivestreamSummary` values, as WP04's `LiveDirectory::streamers`.
                json!({"id": "guest", "displayName": "Guest", "tier": "background",
                       "bindings": [{"platform": "twitch", "username": "guest", "url": "https://www.twitch.tv/guest"}],
                       "dgg": {"hosted": true, "viewers": 300}, "live": true, "title": "Title",
                       "category": null, "viewerCount": 10, "maxViewerCount": 10,
                       "startedAt": 1_767_225_600_000_i64, "lastStartedAt": null, "lastEndedAt": null,
                       "primary": null, "sources": [], "discoverySource": "dgg"}),
                json!({"id": "main", "displayName": "Main", "tier": "primary",
                       "bindings": [{"platform": "kick", "username": "main", "url": "https://kick.com/main"}],
                       "dgg": null, "live": true, "sources": []}),
            ])
        })
    }
    fn statuses(&self) -> BoxFuture<'_, Result<Vec<Value>, PortError>> {
        let statuses = self.statuses.lock().expect("lock").clone();
        Box::pin(async move { Ok(statuses) })
    }
    fn display(&self) -> BoxFuture<'_, Result<Vec<Value>, PortError>> {
        Box::pin(async { Ok(vec![]) })
    }
    fn details<'a>(&'a self, _id: &'a str) -> BoxFuture<'a, Result<Option<Value>, PortError>> {
        Box::pin(async { Ok(None) })
    }
}

/// A `StreamerStatusView` as WP04's `LiveDirectory::statuses` serializes it.
fn live_status(id: &str, platform: &str) -> Value {
    json!({"streamerId": id, "isLive": true,
           "primary": {"platform": platform, "username": id, "url": format!("https://example.test/{id}")},
           "primaryTitle": "Title", "startedAt": 1_767_225_600_000_i64,
           "maxViewerCount": 10, "viewerCount": 10,
           "sources": [{"platform": platform, "username": id, "title": "Title", "viewerCount": 10}]})
}

#[tokio::test]
async fn the_port_observes_due_live_streamers_and_ends_sessions() {
    let harness = Harness::new(Some("user")).await;
    let service = harness.service();
    let ports = Ports::default();
    let directory = Arc::new(Directory {
        statuses: Mutex::new(vec![
            live_status("guest", "twitch"),
            live_status("main", "kick"),
        ]),
    });
    ports
        .set_live_directory(directory.clone())
        .expect("directory");
    let cell = Arc::new(OnceLock::new());
    cell.set(service.clone()).ok();
    let clock: SharedClock = harness.clock.clone();
    let port = IntelligencePort::new(IntelState {
        store: harness.store.clone(),
        clock,
        ports,
        service: cell,
    });
    // Tick 0: both are due (background polls on every third tick).
    port.after_tick().await;
    assert!(service.is_active("guest"));
    assert!(service.is_active("main"));
    let details = port
        .details("guest", 50)
        .await
        .expect("details")
        .expect("some");
    let dto: omni_api::intelligence::IntelligenceDetailsResponse =
        serde_json::from_value(details).expect("matches the omni-api DTO");
    let runtime = dto.runtime.expect("runtime while enabled");
    assert_eq!(runtime.active_stream_count, 2);
    assert_eq!(runtime.active_voice_target_count, 1);
    assert_eq!(runtime.model, "parakeet-tdt-0.6b-v3-int8");
    assert_eq!(runtime.budget.limit_cents, 300.0);
    assert!(dto.diagnostics.is_some());
    // The background streamer goes offline; the transition hook ends its session.
    *directory.statuses.lock().expect("lock") = vec![live_status("main", "kick")];
    port.on_transition(&LiveTransition {
        streamer_id: "guest".into(),
        live: false,
        at_ms: 0,
    })
    .await;
    assert!(!service.is_active("guest"));
    // Main goes offline without a transition call: the next pull ends it.
    *directory.statuses.lock().expect("lock") = vec![];
    port.after_tick().await;
    assert!(!service.is_active("main"));
    service.close().await;
    let events = get_events(&harness.store, Some("guest"), 200)
        .await
        .expect("events");
    assert!(events.iter().any(|e| e.title == "Live session ended"));
    let started = get_intelligence(&harness.store, "main")
        .await
        .expect("read")
        .expect("present")
        .session_started_at;
    assert_eq!(started, 1_767_225_600_000);
}
