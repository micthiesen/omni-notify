//! Port of `src/live-check/metrics/ViewerMetricsService.spec.ts` over a real
//! temporary docstore (the TS mocked the persistence module).
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::sync::Arc;

use omni_live::error::LiveError;
use omni_live::metrics::{
    ViewerMetrics, ViewerMetricsService, ViewerObservation, get_viewer_metrics,
};
use omni_live::notification_policy::ViewerRecordScope;
use omni_live::platform::NotificationUrlFields;
use omni_store::entity::{EntityWrite, UpsertOpts};

fn url_fields() -> NotificationUrlFields {
    NotificationUrlFields {
        url: "https://example.com".into(),
        url_title: "Watch".into(),
    }
}

fn observation(
    id: &str,
    name: &str,
    count: i64,
    token: Option<&str>,
    scope: ViewerRecordScope,
) -> ViewerObservation {
    ViewerObservation {
        streamer_id: id.into(),
        display_name: name.into(),
        viewer_count: count,
        url_fields: url_fields(),
        token: token.map(str::to_owned),
        scope,
    }
}

async fn service(
    now: i64,
) -> (
    omni_testkit::TestStore,
    ViewerMetricsService,
    common::FakeNotifier,
) {
    let (store, _clock) = common::test_store(now).await;
    let notifier = common::FakeNotifier::default();
    let service = ViewerMetricsService::new(
        store.store.clone(),
        common::utc(),
        Arc::new(notifier.clone()),
    );
    (store, service, notifier)
}

async fn seed(store: &omni_store::Store, id: &str, all_time_max: i64) {
    let metrics = ViewerMetrics {
        all_time_max,
        ..ViewerMetrics::empty(id)
    };
    store
        .write(move |tx| tx.upsert(&metrics, UpsertOpts::default()))
        .await
        .unwrap();
}

const T: i64 = 1_790_000_000_000;

#[tokio::test]
async fn notifies_on_a_confirmed_record_with_no_mute_path_records_fire_for_muted_streamers_too() {
    let (_store, service, notifier) = service(T).await;
    service
        .record_viewer_count(
            &observation(
                "muted",
                "Muted",
                100,
                Some("tok-live"),
                ViewerRecordScope::All,
            ),
            T,
        )
        .await
        .unwrap();
    assert_eq!(notifier.count(), 0);
    service
        .record_viewer_count(
            &observation(
                "muted",
                "Muted",
                90,
                Some("tok-live"),
                ViewerRecordScope::All,
            ),
            T,
        )
        .await
        .unwrap();
    let sent = notifier.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].0.as_deref(), Some("tok-live"));
    assert!(sent[0].1.title.contains("Muted"));
    assert_eq!(sent[0].1.url, Some(url_fields()));
    assert_eq!(sent[0].1.title, "New all-time record for Muted!");
    assert_eq!(sent[0].1.message, "Peaked at 100 viewers.");
}

#[tokio::test]
async fn notifies_on_flush_pending_peaks_offline_flush_with_no_mute_path() {
    let (_store, service, notifier) = service(T).await;
    service
        .record_viewer_count(
            &observation("muted", "Muted", 100, None, ViewerRecordScope::All),
            T,
        )
        .await
        .unwrap();
    assert_eq!(notifier.count(), 0);
    service
        .flush_pending_peaks(
            &observation("muted", "Muted", 0, None, ViewerRecordScope::All),
            T,
        )
        .await
        .unwrap();
    assert_eq!(notifier.count(), 1);
    assert!(notifier.titles()[0].contains("Muted"));
}

#[tokio::test]
async fn does_not_notify_while_a_peak_is_still_climbing() {
    let (_store, service, notifier) = service(T).await;
    for count in [100, 150, 149] {
        service
            .record_viewer_count(
                &observation("s", "S", count, None, ViewerRecordScope::All),
                T,
            )
            .await
            .unwrap();
    }
    assert_eq!(notifier.count(), 0);
}

#[tokio::test]
async fn uses_the_effect_clock_for_bucket_and_confirmed_peak_timestamps() {
    let (store, service, _notifier) = service(42_000).await;
    service
        .record_viewer_count(
            &observation("clocked", "Clocked", 100, None, ViewerRecordScope::All),
            42_000,
        )
        .await
        .unwrap();
    service
        .record_viewer_count(
            &observation("clocked", "Clocked", 90, None, ViewerRecordScope::All),
            42_000,
        )
        .await
        .unwrap();
    let metrics = get_viewer_metrics(&store.store, "clocked").await.unwrap();
    assert_eq!(metrics.all_time_max_timestamp, 42_000);
    assert_eq!(metrics.daily_buckets[0].timestamp, 42_000);
}

#[tokio::test]
async fn returns_persistence_writes_as_typed_failures() {
    let (store, service, _notifier) = service(T).await;
    common::break_store(&store.store).await;
    let error = service
        .record_viewer_count(
            &observation("broken", "Broken", 100, None, ViewerRecordScope::All),
            T,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, LiveError::Persistence { .. }), "{error}");
}

#[tokio::test]
async fn suppresses_window_only_confirmations_7d_30d_90d_when_scope_is_all_time_only() {
    let (store, service, notifier) = service(T).await;
    seed(&store.store, "bg", 500).await;
    let scope = ViewerRecordScope::AllTimeOnly;
    service
        .record_viewer_count(&observation("bg", "Background", 50, Some("tok"), scope), T)
        .await
        .unwrap();
    assert_eq!(notifier.count(), 0);
    service
        .record_viewer_count(&observation("bg", "Background", 40, Some("tok"), scope), T)
        .await
        .unwrap();
    assert_eq!(notifier.count(), 0);
    let persisted = get_viewer_metrics(&store.store, "bg").await.unwrap();
    assert_eq!(persisted.daily_buckets.last().unwrap().max_viewers, 50);
    assert_eq!(persisted.all_time_max, 500);
}

#[tokio::test]
async fn still_notifies_a_genuine_all_time_record_when_scope_is_all_time_only() {
    let (_store, service, notifier) = service(T).await;
    let scope = ViewerRecordScope::AllTimeOnly;
    service
        .record_viewer_count(
            &observation("bg2", "Background2", 100, Some("tok"), scope),
            T,
        )
        .await
        .unwrap();
    assert_eq!(notifier.count(), 0);
    service
        .record_viewer_count(
            &observation("bg2", "Background2", 90, Some("tok"), scope),
            T,
        )
        .await
        .unwrap();
    assert_eq!(notifier.count(), 1);
    assert!(notifier.titles()[0].contains("all-time record"));
}

#[tokio::test]
async fn suppresses_a_window_only_flush_on_went_offline_when_scope_is_all_time_only() {
    let (store, service, notifier) = service(T).await;
    seed(&store.store, "bg3", 500).await;
    let scope = ViewerRecordScope::AllTimeOnly;
    service
        .record_viewer_count(
            &observation("bg3", "Background3", 50, Some("tok"), scope),
            T,
        )
        .await
        .unwrap();
    service
        .flush_pending_peaks(&observation("bg3", "Background3", 0, Some("tok"), scope), T)
        .await
        .unwrap();
    assert_eq!(notifier.count(), 0);
}

#[tokio::test]
async fn formats_the_previous_record_with_grouping() {
    let (store, service, notifier) = service(T).await;
    seed(&store.store, "big", 12_000).await;
    service
        .record_viewer_count(
            &observation("big", "Big", 15_000, None, ViewerRecordScope::All),
            T,
        )
        .await
        .unwrap();
    service
        .record_viewer_count(
            &observation("big", "Big", 1_000, None, ViewerRecordScope::All),
            T,
        )
        .await
        .unwrap();
    let sent = notifier.sent();
    assert_eq!(
        sent[0].1.message,
        "Peaked at 15,000 viewers (previous: 12,000)."
    );
}
