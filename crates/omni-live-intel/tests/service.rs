//! The livestream intelligence service and its work queue.
//!
//! `WorkQueue::fork` admits synchronously (there is no await between the
//! admission check and the spawn), so the admission case asserts that nothing
//! leaks after fork, release and close. The service cases run on a fixed clock
//! and wait for detached work by polling the Pushover recorder.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::Duration;

use futures::future::BoxFuture;
use omni_ai::costs::{CostEventData, CostRecorder};
use omni_alerts::{Pushover, PushoverChannel};
use omni_core::clock::{Clock, SharedClock};
use omni_http::SideEffectMode;
use omni_live_intel::audio::{AudioError, AudioSource, CapturedAudio};
use omni_live_intel::classifier::{
    AssessmentError, TranscriptAssessment, TranscriptAssessmentInput, TranscriptClassifier,
};
use omni_live_intel::observation::{
    DggPresence, LiveObservation, LiveStatus, Platform, PlatformBinding, SourceObservation,
    Streamer, StreamerTier, viewer_count_for_anomaly,
};
use omni_live_intel::persistence::{
    DESTINY_CONFIRMED_EVENT_TITLE, NewLivestreamEvent, get_intelligence, record_event,
    save_intelligence,
};
use omni_live_intel::service::{
    LivestreamIntelligenceService, ServiceDeps, ServiceSettings, is_destiny_owned_stream,
};
use omni_live_intel::speech::{SpeakerMatch, SpeechEngine, SpeechRecognitionError};
use omni_live_intel::types::{
    DestinyPresence, EventStatus, LivestreamAlertRecord, LivestreamAlertType, LivestreamEventKind,
    LivestreamIntelligenceData, Metrics, PresenceState, metric_number,
};
use omni_live_intel::work_queue::WorkQueue;
use omni_store::Store;
use omni_store::entity::EntityOps as _;
use omni_testkit::TestStore;
use tokio::sync::oneshot;
use tokio_util::task::TaskTracker;

async fn wait_for(mut condition: impl FnMut() -> bool) {
    for _ in 0..500 {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("condition not reached");
}

// ---- EffectWorkQueue close ----

#[tokio::test]
async fn closes_admission_before_draining_work_already_accepted() {
    let queue = WorkQueue::new("test", 1, TaskTracker::new());
    let (release, released) = oneshot::channel::<()>();
    queue.fork(async move {
        let _ = released.await;
    });
    let closing = tokio::spawn({
        let queue = queue.clone();
        async move { queue.close().await }
    });
    wait_for(|| queue.running() == 1).await;
    assert!(queue.run(async {}).await.is_err());
    assert!(!closing.is_finished());
    release.send(()).expect("release");
    closing.await.expect("closed");
    assert_eq!(queue.running(), 0);
    assert_eq!(queue.queued(), 0);
}

#[tokio::test]
async fn drains_an_admitted_job_interrupted_while_waiting_for_a_permit() {
    let queue = WorkQueue::new("test", 1, TaskTracker::new());
    let (release, released) = oneshot::channel::<()>();
    queue.fork(async move {
        let _ = released.await;
    });
    wait_for(|| queue.running() == 1).await;
    let waiting = tokio::spawn({
        let queue = queue.clone();
        async move {
            let _ = queue.run(futures::future::pending::<()>()).await;
        }
    });
    wait_for(|| queue.queued() == 1).await;
    let closing = tokio::spawn({
        let queue = queue.clone();
        async move { queue.close().await }
    });
    waiting.abort();
    let _ = waiting.await;
    assert_eq!(queue.queued(), 0);
    release.send(()).expect("release");
    closing.await.expect("closed");
    assert_eq!(queue.running(), 0);
}

#[tokio::test]
async fn cannot_leak_admission_when_fork_is_interrupted_during_startup() {
    let queue = WorkQueue::new("test", 1, TaskTracker::new());
    let (release, released) = oneshot::channel::<()>();
    queue.fork(async move {
        let _ = released.await;
    });
    release.send(()).expect("release");
    queue.close().await;
    assert_eq!(queue.running(), 0);
    assert_eq!(queue.queued(), 0);
}

#[tokio::test(start_paused = true)]
async fn interrupts_a_hung_detached_job_after_the_graceful_drain_window() {
    struct Finalizer(Arc<AtomicBool>);
    impl Drop for Finalizer {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let queue = WorkQueue::new("test", 1, TaskTracker::new());
    let finalized = Arc::new(AtomicBool::new(false));
    let guard = Finalizer(Arc::clone(&finalized));
    queue.fork(async move {
        let _guard = guard;
        futures::future::pending::<()>().await;
    });
    tokio::task::yield_now().await;
    queue.close().await;
    assert!(finalized.load(Ordering::SeqCst));
    assert_eq!(queue.running(), 0);
}

// ---- isDestinyOwnedStream ----

fn candidate_streamer() -> Streamer {
    Streamer {
        id: "dgg:youtube:video-id".into(),
        display_name: "Guest Channel".into(),
        bindings: vec![PlatformBinding::new(Platform::YouTube, "video-id")],
        tier: StreamerTier::Background,
        dgg: Some(DggPresence {
            hosted: false,
            viewers: Some(100.0),
        }),
    }
}

#[test]
fn rejects_the_configured_destiny_stream() {
    assert!(is_destiny_owned_stream(&Streamer {
        id: "destiny".into(),
        ..candidate_streamer()
    }));
}

#[test]
fn rejects_a_dgg_discovered_destiny_video_with_a_dynamic_id() {
    assert!(is_destiny_owned_stream(&Streamer {
        id: "dgg:youtube:abc123".into(),
        display_name: "Destiny".into(),
        ..candidate_streamer()
    }));
}

#[test]
fn rejects_canonical_destiny_usernames_regardless_of_display_name() {
    assert!(is_destiny_owned_stream(&Streamer {
        bindings: vec![PlatformBinding::new(Platform::YouTube, "@Destiny")],
        ..candidate_streamer()
    }));
}

#[test]
fn keeps_a_third_party_dgg_stream_eligible() {
    assert!(!is_destiny_owned_stream(&candidate_streamer()));
}

// ---- viewerCountForAnomaly ----

const SESSION_STARTED_AT: i64 = 10_000;

fn darius() -> Streamer {
    Streamer {
        id: "darius".into(),
        display_name: "Darius".into(),
        bindings: vec![PlatformBinding::new(Platform::Kick, "dariusirl")],
        tier: StreamerTier::Background,
        dgg: Some(DggPresence {
            viewers: Some(599.0),
            hosted: true,
        }),
    }
}

fn status() -> LiveStatus {
    LiveStatus {
        streamer_id: "darius".into(),
        primary: PlatformBinding::new(Platform::Kick, "dariusirl"),
        primary_title: "Live debate".into(),
        started_at: SESSION_STARTED_AT,
        viewer_count: Some(700.0),
        sources: None,
    }
}

fn source(platform: Platform, username: &str, viewers: Option<f64>) -> SourceObservation {
    SourceObservation {
        platform,
        username: username.into(),
        title: "Live debate".into(),
        viewer_count: viewers,
    }
}

#[test]
fn uses_the_sticky_primary_source_instead_of_summing_overlapping_bindings() {
    let status = LiveStatus {
        viewer_count: Some(1_700.0),
        sources: Some(vec![
            source(Platform::Kick, "dariusirl", Some(700.0)),
            source(Platform::YouTube, "darius", Some(1_000.0)),
        ]),
        ..status()
    };
    assert_eq!(viewer_count_for_anomaly(&status), Some(700.0));
}

#[test]
fn falls_back_to_the_aggregate_for_records_without_source_observations() {
    assert_eq!(viewer_count_for_anomaly(&status()), Some(700.0));
}

#[test]
fn does_not_substitute_secondary_viewers_when_the_primary_count_is_missing() {
    let status = LiveStatus {
        viewer_count: Some(1_000.0),
        sources: Some(vec![
            source(Platform::Kick, "dariusirl", None),
            source(Platform::YouTube, "darius", Some(1_000.0)),
        ]),
        ..status()
    };
    assert_eq!(viewer_count_for_anomaly(&status), None);
}

// ---- service fixtures ----

struct FixedClock(AtomicI64);

impl Clock for FixedClock {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

struct FakeCapture;

impl AudioSource for FakeCapture {
    fn capture<'a>(
        &'a self,
        _url: &'a str,
        _seconds: u32,
    ) -> BoxFuture<'a, Result<CapturedAudio, AudioError>> {
        Box::pin(async {
            Ok(CapturedAudio {
                samples: vec![0.0; 18 * 16_000],
                sample_rate: 16_000,
                duration_seconds: 18.0,
            })
        })
    }
}

struct FakeSpeech;

impl SpeechEngine for FakeSpeech {
    fn has_voiceprint(&self) -> bool {
        true
    }

    fn detect_destiny(
        &self,
        _samples: Arc<Vec<f32>>,
    ) -> BoxFuture<'_, Result<SpeakerMatch, SpeechRecognitionError>> {
        Box::pin(async {
            Ok(SpeakerMatch {
                confidence: 0.755,
                matched_windows: 1,
                checked_windows: 4,
            })
        })
    }

    fn transcribe(
        &self,
        _samples: Arc<Vec<f32>>,
    ) -> BoxFuture<'_, Result<String, SpeechRecognitionError>> {
        Box::pin(async { Ok("unused transcript".to_owned()) })
    }
}

struct NoClassifier;

impl TranscriptClassifier for NoClassifier {
    fn assess(
        &self,
        _input: TranscriptAssessmentInput,
    ) -> BoxFuture<'_, Result<Option<TranscriptAssessment>, AssessmentError>> {
        Box::pin(async { panic!("the classifier is not reached in these cases") })
    }
}

struct Harness {
    _store: TestStore,
    store: Store,
    clock: Arc<FixedClock>,
    pushover: Pushover,
}

impl Harness {
    async fn new(now: i64) -> Self {
        let clock = Arc::new(FixedClock(AtomicI64::new(now)));
        let shared: SharedClock = clock.clone();
        let test_store = TestStore::new(shared).await;
        let store = test_store.store.clone();
        let pushover = Pushover::with_credentials(
            omni_testkit::no_network(),
            Some("test-user".into()),
            [(PushoverChannel::Live, "fake-live-token".to_owned())],
            SideEffectMode::Record,
        );
        Self {
            _store: test_store,
            store,
            clock,
            pushover,
        }
    }

    fn set_now(&self, now: i64) {
        self.clock.0.store(now, Ordering::SeqCst);
    }

    fn service(&self) -> LivestreamIntelligenceService {
        let clock: SharedClock = self.clock.clone();
        LivestreamIntelligenceService::new(ServiceDeps {
            store: self.store.clone(),
            clock: clock.clone(),
            pushover: self.pushover.clone(),
            costs: CostRecorder::new(self.store.clone(), clock),
            capture: Arc::new(FakeCapture),
            speech: Arc::new(FakeSpeech),
            classifier: Arc::new(NoClassifier),
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

    async fn intelligence(&self) -> LivestreamIntelligenceData {
        get_intelligence(&self.store, "darius")
            .await
            .expect("read")
            .expect("present")
    }
}

fn observation(streamer: Streamer) -> LiveObservation {
    LiveObservation {
        streamer,
        status: status(),
    }
}

#[tokio::test]
async fn retries_a_previously_confirmed_alert_once_and_persists_durable_dedup() {
    let harness = Harness::new(1_767_225_600_000).await;
    save_intelligence(
        &harness.store,
        LivestreamIntelligenceData {
            streamer_id: "darius".into(),
            session_started_at: SESSION_STARTED_AT,
            relevance_score: 20.0,
            relevance_reasons: vec![],
            chapters: vec![],
            updated_at: 20_000,
            semantic: None,
            trend: None,
            summary: None,
            destiny_presence: Some(DestinyPresence {
                state: PresenceState::Possible,
                confidence: 0.765,
                detected_at: 20_000,
                reason: "Awaiting confirmation".into(),
                extra: Default::default(),
            }),
            latest_alert: None,
            alerted_at_by_type: None,
            extra: Default::default(),
        },
    )
    .await
    .expect("save");
    let mut metrics = Metrics::new();
    metrics.insert(
        "speakerConfidence".into(),
        metric_number(0.705_942_983_138_825_1),
    );
    metrics.insert("assessmentConfidence".into(), metric_number(0.91));
    record_event(
        &harness.store,
        NewLivestreamEvent::new(
            "darius",
            Some(SESSION_STARTED_AT),
            LivestreamEventKind::Voice,
            EventStatus::Success,
            DESTINY_CONFIRMED_EVENT_TITLE,
        )
        .detail("Destiny is participating in the live conversation.")
        .metrics(metrics)
        .created_at(21_000),
    )
    .await
    .expect("event");

    let first = harness.service();
    first
        .observe_live(observation(darius()))
        .await
        .expect("observe");
    first.after_tick().await.expect("tick");
    first.close().await;

    let pushes = harness.pushover.recorded();
    assert_eq!(pushes.len(), 1);
    assert_eq!(
        pushes[0].message.title.as_deref(),
        Some("Destiny is on Darius")
    );
    assert_eq!(pushes[0].token, "fake-live-token");
    let delivered = harness.intelligence().await;
    assert_eq!(
        delivered.destiny_presence.as_ref().map(|p| p.state),
        Some(PresenceState::Confirmed)
    );
    assert_eq!(
        delivered.latest_alert.as_ref().map(|a| a.alert_type),
        Some(LivestreamAlertType::DestinyGuest)
    );
    assert!(
        delivered
            .alerted_at_by_type
            .as_ref()
            .is_some_and(|m| m.contains_key("destiny_guest"))
    );

    save_intelligence(
        &harness.store,
        LivestreamIntelligenceData {
            latest_alert: Some(LivestreamAlertRecord {
                alert_id: "later-alert".into(),
                alert_type: LivestreamAlertType::Debate,
                title: "Debate".into(),
                message: "A debate started".into(),
                reason: "Transcript evidence".into(),
                confidence: 0.9,
                created_at: harness.clock.now_ms(),
                extra: Default::default(),
            }),
            ..delivered
        },
    )
    .await
    .expect("save");
    let second = harness.service();
    second
        .observe_live(observation(darius()))
        .await
        .expect("observe");
    second.after_tick().await.expect("tick");
    second.close().await;
    assert_eq!(harness.pushover.recorded().len(), 1);
}

#[tokio::test]
async fn records_local_transcription_usage_when_summary_audio_is_processed() {
    let harness = Harness::new(1_767_225_600_000).await;
    let service = harness.service();
    service
        .observe_live(observation(Streamer {
            tier: StreamerTier::Primary,
            ..darius()
        }))
        .await
        .expect("observe");
    service.close().await;
    let costs = harness
        .store
        .read(|docs| docs.get_all::<CostEventData>())
        .await
        .expect("costs");
    let event = costs
        .iter()
        .find(|e| e.operation == "rolling-summary")
        .expect("transcription cost recorded");
    assert_eq!(
        serde_json::to_value(event.category).expect("json"),
        "transcription"
    );
    assert_eq!(event.feature, "livestream-intelligence");
    assert_eq!(event.service, "self-hosted");
    assert_eq!(event.cost_cents, Some(0.0));
}

#[tokio::test]
async fn sends_one_durable_alert_for_a_sustained_late_primary_source_surge() {
    let clock_base = 2_000_000_000_000_i64;
    let harness = Harness::new(clock_base).await;
    let primary = Streamer {
        tier: StreamerTier::Primary,
        ..darius()
    };
    let make = |viewers: f64| LiveObservation {
        streamer: primary.clone(),
        status: LiveStatus {
            viewer_count: Some(viewers + 1_000.0),
            sources: Some(vec![
                source(Platform::Kick, "dariusirl", Some(viewers)),
                source(Platform::YouTube, "darius", Some(1_000.0)),
            ]),
            ..status()
        },
    };
    let observe_at = |service: &LivestreamIntelligenceService, minute: i64, viewers: f64| {
        harness.set_now(clock_base + minute * 60_000);
        let service = service.clone();
        let observation = make(viewers);
        async move { service.observe_live(observation).await.expect("observe") }
    };

    let first = harness.service();
    for minute in 0..15 {
        observe_at(&first, minute, 200.0).await;
    }
    observe_at(&first, 16, 430.0).await;
    assert!(harness.pushover.recorded().is_empty());
    observe_at(&first, 17, 440.0).await;
    let state = harness.intelligence().await;
    assert_eq!(state.relevance_score, 79.0);
    let trend = state.trend.expect("trend");
    assert!(trend.anomalous);
    assert_eq!(trend.baseline_viewers, Some(Some(200.0)));
    assert_eq!(trend.candidate_observations, Some(2.0));
    wait_for(|| harness.pushover.recorded().len() == 1).await;
    let push = &harness.pushover.recorded()[0];
    assert_eq!(push.message.title.as_deref(), Some("Darius is surging"));
    assert!(push.message.message.contains("440 vs 200 baseline"));
    first.close().await;
    assert!(
        harness
            .intelligence()
            .await
            .alerted_at_by_type
            .is_some_and(|m| m.contains_key("viewer_surge"))
    );

    let restarted = harness.service();
    for minute in 40..55 {
        observe_at(&restarted, minute, 200.0).await;
    }
    observe_at(&restarted, 55, 430.0).await;
    observe_at(&restarted, 56, 440.0).await;
    restarted.close().await;
    assert_eq!(harness.pushover.recorded().len(), 1);
}
