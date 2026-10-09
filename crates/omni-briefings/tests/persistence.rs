//! Port of `src/briefing-agent/persistence.spec.ts`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_briefings::format::local_ms;
use omni_briefings::persistence::{
    BriefingNotificationData, CostCents, add_notification, complete_delivery, distribute_run_cost,
    format_notifications, get_history, release_delivery, reserve_delivery,
    resolve_history_placeholders,
};
use omni_testkit::{TEST_EPOCH_MS, TestStore, test_clock};

fn tz() -> jiff::tz::TimeZone {
    jiff::tz::TimeZone::get("America/Vancouver").unwrap()
}

fn notification(title: &str) -> BriefingNotificationData {
    BriefingNotificationData::new(
        title.to_owned(),
        "msg".to_owned(),
        "https://example.com".to_owned(),
        local_ms(&tz(), jiff::civil::date(2026, 2, 6).at(14, 30, 0, 0)).unwrap(),
    )
}

async fn store() -> TestStore {
    TestStore::new(test_clock(TEST_EPOCH_MS)).await
}

#[test]
fn formats_and_bounds_recent_notifications() {
    assert_eq!(
        format_notifications(&[], 5, &tz()),
        "- No previous notifications"
    );
    let result = format_notifications(&[notification("Old"), notification("Recent")], 1, &tz());
    assert!(!result.contains("Old"));
    assert!(result.contains("Recent"));
    assert_eq!(result, "- Recent (https://example.com) [Feb 6, 2:30 PM]");
}

#[tokio::test]
async fn appends_and_prunes_notification_history() {
    let s = store().await;
    for index in 0..55 {
        add_notification(&s.store, "News", notification(&format!("N{index}")))
            .await
            .unwrap();
    }
    let history = get_history(&s.store, "News").await.unwrap();
    assert_eq!(history.notifications.len(), 50);
    assert_eq!(history.notifications[0].title, "N5");
}

#[tokio::test]
async fn preserves_concurrent_notification_appends() {
    let s = store().await;
    futures::future::join_all((0..20).map(|index| {
        let store = s.store.clone();
        async move {
            add_notification(&store, "News", notification(&format!("N{index}")))
                .await
                .unwrap();
        }
    }))
    .await;
    let history = get_history(&s.store, "News").await.unwrap();
    assert_eq!(history.notifications.len(), 20);
    let titles: std::collections::HashSet<_> = history
        .notifications
        .iter()
        .map(|n| n.title.clone())
        .collect();
    assert_eq!(titles.len(), 20);
}

#[tokio::test]
async fn does_not_lose_an_append_concurrent_with_cost_distribution() {
    let s = store().await;
    let mut costed = notification("Costed");
    costed.run_id = Some("run-1".to_owned());
    add_notification(&s.store, "News", costed).await.unwrap();
    let (a, b) = tokio::join!(
        distribute_run_cost(&s.store, "News", Some("run-1"), Some(12.0)),
        add_notification(&s.store, "News", notification("Concurrent")),
    );
    a.unwrap();
    b.unwrap();
    let history = get_history(&s.store, "News").await.unwrap();
    let titles: Vec<&str> = history
        .notifications
        .iter()
        .map(|n| n.title.as_str())
        .collect();
    assert_eq!(titles, ["Costed", "Concurrent"]);
    assert_eq!(history.notifications[0].cost_cents, CostCents::Cents(12.0));
}

#[tokio::test]
async fn reserves_delivery_atomically_and_permits_retry_after_release() {
    let s = store().await;
    assert!(
        reserve_delivery(&s.store, "News", "run:hash")
            .await
            .unwrap()
    );
    complete_delivery(&s.store, "News", "run:hash")
        .await
        .unwrap();
    assert!(
        !reserve_delivery(&s.store, "News", "run:hash")
            .await
            .unwrap()
    );
    release_delivery(&s.store, "News", "run:hash")
        .await
        .unwrap();
    assert!(
        reserve_delivery(&s.store, "News", "run:hash")
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn resolves_every_history_placeholder() {
    let s = store().await;
    add_notification(&s.store, "News", notification("Article"))
        .await
        .unwrap();
    let result = resolve_history_placeholders(
        &s.store,
        "A: {{history:3}}\nB: {{history:0}}",
        "News",
        &tz(),
    )
    .await
    .unwrap();
    assert!(result.contains("Article"));
    assert!(result.contains("No previous notifications"));
    assert!(!result.contains("{{history"));
}

#[tokio::test]
async fn unpriced_runs_store_null_and_old_rows_stay_absent() {
    let s = store().await;
    let mut first = notification("First");
    first.run_id = Some("run-a".to_owned());
    add_notification(&s.store, "News", first).await.unwrap();
    add_notification(&s.store, "News", notification("Legacy"))
        .await
        .unwrap();
    distribute_run_cost(&s.store, "News", Some("run-a"), None)
        .await
        .unwrap();
    let history = get_history(&s.store, "News").await.unwrap();
    assert_eq!(history.notifications[0].cost_cents, CostCents::Unpriced);
    assert_eq!(history.notifications[1].cost_cents, CostCents::Absent);
    // A run that produced nothing leaves every row untouched.
    distribute_run_cost(&s.store, "News", Some("missing"), Some(5.0))
        .await
        .unwrap();
    let again = get_history(&s.store, "News").await.unwrap();
    assert_eq!(again, history);
}
