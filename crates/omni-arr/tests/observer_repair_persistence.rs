//! Port of `src/observer-repair/persistence.spec.ts` against a file-backed test store.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_arr::observer_repair::persistence::{
    OBSERVER_REPAIR_LEASE_MS, RepairOutcome, RepairPhase, acquire_issue, list_pending, save_issue,
};
use omni_store::Store;
use omni_testkit::{TestStore, test_clock};

const NOW: i64 = 1_800_000_000_000;

#[tokio::test]
async fn allows_only_one_worker_to_reserve_an_issue() {
    let test = TestStore::new(test_clock(NOW)).await;
    let store: &Store = &test.store;
    let attempts = (0..10).map(|index| {
        let owner = format!("worker-{index}");
        async move { acquire_issue(store, 42, "revision-1", &owner, NOW).await }
    });
    let reserved = futures::future::join_all(attempts)
        .await
        .into_iter()
        .map(Result::unwrap)
        .filter(Option::is_some)
        .count();
    assert_eq!(reserved, 1);
}

#[tokio::test]
async fn permits_a_new_owner_after_lease_expiry_and_marks_interrupted_execution_unhandled() {
    let test = TestStore::new(test_clock(NOW)).await;
    let mut first = acquire_issue(&test.store, 7, "r1", "first", NOW)
        .await
        .unwrap()
        .unwrap();
    first.phase = RepairPhase::Executing;
    save_issue(&test.store, &first, "first", NOW + 1)
        .await
        .unwrap();
    let resumed = acquire_issue(
        &test.store,
        7,
        "r1",
        "second",
        NOW + OBSERVER_REPAIR_LEASE_MS,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(resumed.phase, RepairPhase::Unhandled);
    assert_eq!(resumed.outcome, Some(RepairOutcome::Unhandled));
    assert_eq!(resumed.lease.unwrap().owner, "second");
}

#[tokio::test]
async fn skips_completed_revisions_and_resets_only_completed_state_for_a_new_revision() {
    let test = TestStore::new(test_clock(NOW)).await;
    let mut first = acquire_issue(&test.store, 9, "r1", "worker", NOW)
        .await
        .unwrap()
        .unwrap();
    first.phase = RepairPhase::Done;
    first.outcome = Some(RepairOutcome::Repaired);
    save_issue(&test.store, &first, "worker", NOW + 1)
        .await
        .unwrap();
    assert!(
        acquire_issue(&test.store, 9, "r1", "other", NOW + 2)
            .await
            .unwrap()
            .is_none()
    );
    let next = acquire_issue(&test.store, 9, "r2", "other", NOW + 2)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(next.phase, RepairPhase::Reserved);
    assert_eq!(next.revision, "r2");
    let active = acquire_issue(&test.store, 10, "r1", "worker", NOW)
        .await
        .unwrap()
        .unwrap();
    assert!(
        acquire_issue(&test.store, 10, "r2", "other", NOW + 1)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(active.revision, "r1");
}

#[tokio::test]
async fn lists_at_most_100_unfinished_states() {
    let test = TestStore::new(test_clock(NOW)).await;
    for issue_id in 1..=101 {
        acquire_issue(&test.store, issue_id, "r1", "worker", NOW)
            .await
            .unwrap();
    }
    let mut first = acquire_issue(&test.store, 1000, "r1", "worker", NOW)
        .await
        .unwrap()
        .unwrap();
    first.phase = RepairPhase::Done;
    save_issue(&test.store, &first, "worker", NOW + 1)
        .await
        .unwrap();
    assert_eq!(list_pending(&test.store).await.unwrap().len(), 100);
}
