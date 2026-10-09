//! Port of `src/parcel-tracker/persistence.atomic.spec.ts`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_core::clock::TestClock;
use omni_parcel::persistence::{self, DeliveryAttempt};

#[tokio::test]
async fn increments_attempts_atomically_across_concurrent_reservations() {
    let store = omni_testkit::TestStore::new(TestClock::new(1_800_000_000_000)).await;
    let attempt = DeliveryAttempt {
        tracking_number: "1Z999AA10123456784".to_owned(),
        carrier_code: "ups".to_owned(),
        description: "Camera".to_owned(),
        submitted_at: 1_800_000_000_000,
        email_id: "email-1".to_owned(),
    };
    let reservations = futures::future::join_all(
        (0..20).map(|_| persistence::reserve(&store.store, attempt.clone())),
    )
    .await;
    let mut attempts: Vec<i64> = reservations
        .into_iter()
        .map(|r| r.unwrap().attempts.unwrap())
        .collect();
    attempts.sort_unstable();
    assert_eq!(attempts, (1..=20).collect::<Vec<_>>());
    let row = persistence::get(&store.store, "1Z999AA10123456784")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.attempts, Some(20));
}
