//! Livestream intelligence persistence, including timeline pruning.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use omni_live_intel::persistence::{
    DESTINY_CONFIRMED_EVENT_TITLE, NewLivestreamEvent, build_feedback_digest, count_events,
    get_diagnostics, get_events, get_latest_destiny_confirmation, record_event, record_feedback,
    save_intelligence, update_stage,
};
use omni_live_intel::types::{
    EventStatus, LivestreamAlertRecord, LivestreamAlertType, LivestreamEventKind,
    LivestreamFeedbackVerdict, LivestreamIntelligenceData, PipelineStage, PipelineStatus,
    StageDiagnostic, metric_number,
};
use omni_store::Store;
use omni_testkit::{TestStore, test_clock};

async fn store() -> (TestStore, Store) {
    let test = TestStore::new(test_clock(1_700_000_000_000)).await;
    let store = test.store.clone();
    (test, store)
}

#[tokio::test]
async fn records_feedback_only_for_the_latest_alert() {
    let (_guard, store) = store().await;
    save_intelligence(
        &store,
        LivestreamIntelligenceData {
            streamer_id: "hutch".into(),
            session_started_at: 1,
            relevance_score: 80.0,
            relevance_reasons: vec![],
            chapters: vec![],
            updated_at: 2,
            semantic: None,
            trend: None,
            summary: None,
            destiny_presence: None,
            latest_alert: Some(LivestreamAlertRecord {
                alert_id: "alert-1".into(),
                alert_type: LivestreamAlertType::Debate,
                title: "Debate".into(),
                message: "Starting".into(),
                reason: "Evidence".into(),
                confidence: 0.9,
                created_at: 2,
                extra: Default::default(),
            }),
            alerted_at_by_type: None,
            extra: Default::default(),
        },
    )
    .await
    .expect("save");
    assert!(
        record_feedback(
            &store,
            "hutch",
            "stale",
            LivestreamFeedbackVerdict::FalsePositive,
            None
        )
        .await
        .expect("record")
        .is_none()
    );
    let feedback = record_feedback(
        &store,
        "hutch",
        "alert-1",
        LivestreamFeedbackVerdict::Useful,
        Some("  exactly what I wanted  "),
    )
    .await
    .expect("record")
    .expect("recorded");
    assert_eq!(feedback.note.as_deref(), Some("exactly what I wanted"));
    assert_eq!(
        build_feedback_digest(&store, 20).await.expect("digest"),
        "debate: useful (exactly what I wanted)"
    );
    let events = get_events(&store, Some("hutch"), 10).await.expect("events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].title, "Alert marked useful");
}

#[tokio::test]
async fn merges_stages_within_a_session_and_resets_for_a_new_session() {
    let (_guard, store) = store().await;
    let mut metadata = StageDiagnostic::new(PipelineStatus::Success);
    metadata.detail = Some("Politics".into());
    update_stage(
        &store,
        "pisco",
        Some(100),
        PipelineStage::Metadata,
        metadata,
    )
    .await
    .expect("stage");
    let mut summary = StageDiagnostic::new(PipelineStatus::Running);
    summary.started_at = Some(120);
    update_stage(&store, "pisco", Some(100), PipelineStage::Summary, summary)
        .await
        .expect("stage");
    let stages = get_diagnostics(&store, "pisco")
        .await
        .expect("read")
        .expect("present")
        .stages;
    assert_eq!(stages["metadata"].status, PipelineStatus::Success);
    assert_eq!(stages["summary"].status, PipelineStatus::Running);

    let mut voice = StageDiagnostic::new(PipelineStatus::Idle);
    voice.eligible = Some(false);
    update_stage(&store, "pisco", Some(200), PipelineStage::Voice, voice)
        .await
        .expect("stage");
    let diagnostics = get_diagnostics(&store, "pisco")
        .await
        .expect("read")
        .expect("present");
    assert_eq!(diagnostics.session_started_at, Some(200));
    assert_eq!(diagnostics.stages.len(), 1);
    assert_eq!(diagnostics.stages["voice"].status, PipelineStatus::Idle);
}

#[tokio::test]
async fn returns_a_bounded_timeline_and_durable_confirmation() {
    let (_guard, store) = store().await;
    let mut metrics = omni_live_intel::types::Metrics::new();
    metrics.insert("speakerConfidence".into(), metric_number(0.706));
    record_event(
        &store,
        NewLivestreamEvent::new(
            "darius",
            Some(100),
            LivestreamEventKind::Voice,
            EventStatus::Success,
            DESTINY_CONFIRMED_EVENT_TITLE,
        )
        .detail("Live conversation confirmed")
        .metrics(metrics)
        .created_at(200),
    )
    .await
    .expect("record");
    assert_eq!(
        get_events(&store, Some("darius"), 1)
            .await
            .expect("events")
            .len(),
        1
    );
    let confirmation = get_latest_destiny_confirmation(&store, "darius", 100)
        .await
        .expect("read")
        .expect("found");
    assert_eq!(
        confirmation.detail.as_deref(),
        Some("Live conversation confirmed")
    );
    assert!(
        get_latest_destiny_confirmation(&store, "darius", 300)
            .await
            .expect("read")
            .is_none()
    );
}

#[tokio::test]
async fn prunes_the_oldest_events_above_the_timeline_cap() {
    let (_guard, store) = store().await;
    for index in 0..3_001_i64 {
        record_event(
            &store,
            NewLivestreamEvent::new(
                "x",
                None,
                LivestreamEventKind::Session,
                EventStatus::Info,
                "event",
            )
            .created_at(index),
        )
        .await
        .expect("record");
    }
    assert_eq!(count_events(&store).await.expect("count"), 2_751);
    let oldest = get_events(&store, None, 200).await.expect("events");
    assert_eq!(oldest[0].created_at, 3_000);
    let all = store
        .read(|docs| {
            use omni_store::entity::EntityOps as _;
            docs.get_all::<omni_live_intel::types::LivestreamIntelligenceEventData>()
        })
        .await
        .expect("all");
    assert_eq!(all.iter().map(|e| e.created_at).min(), Some(250));
}

#[tokio::test]
async fn event_limits_are_clamped() {
    let (_guard, store) = store().await;
    for index in 0..3_i64 {
        record_event(
            &store,
            NewLivestreamEvent::new(
                "y",
                None,
                LivestreamEventKind::Alert,
                EventStatus::Info,
                "e",
            )
            .created_at(index),
        )
        .await
        .expect("record");
    }
    assert_eq!(
        get_events(&store, Some("y"), 0)
            .await
            .expect("events")
            .len(),
        1
    );
    assert_eq!(
        get_events(&store, Some("y"), 999)
            .await
            .expect("events")
            .len(),
        3
    );
}
