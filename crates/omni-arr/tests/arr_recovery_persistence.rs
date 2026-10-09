//! Port of `src/arr-recovery/persistence.spec.ts` against a file-backed test store.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_arr::arr_recovery::ArrKind;
use omni_arr::arr_recovery::persistence::{
    Observation, RECOVERY_LEASE_MS, StoredRecoveryState, acquire_state, release_state, save_state,
};
use omni_store::cbor;
use omni_store::entity::{self, Entity as _};
use omni_store::{DocMeta, DocWrite as _, Store, StoreError};
use omni_testkit::{TestStore, test_clock};
use serde_json::json;

const NOW: i64 = 1_800_000_000_000;

async fn store() -> TestStore {
    TestStore::new(test_clock(NOW)).await
}

#[tokio::test]
async fn acquires_only_one_lease_for_concurrent_workers_of_the_same_kind() {
    let test = store().await;
    let store: &Store = &test.store;
    let attempts = (0..20).map(|index| {
        let owner = format!("worker-{index}");
        async move { acquire_state(store, ArrKind::Sonarr, &owner, NOW).await }
    });
    let states = futures::future::join_all(attempts).await;
    let acquired = states
        .into_iter()
        .map(Result::unwrap)
        .filter(Option::is_some)
        .count();
    assert_eq!(acquired, 1);
}

#[tokio::test]
async fn leases_sonarr_and_radarr_independently() {
    let test = store().await;
    let (sonarr, radarr) = tokio::join!(
        acquire_state(&test.store, ArrKind::Sonarr, "sonarr-worker", NOW),
        acquire_state(&test.store, ArrKind::Radarr, "radarr-worker", NOW),
    );
    assert_eq!(
        sonarr.unwrap().unwrap().value.lease.unwrap().owner,
        "sonarr-worker"
    );
    assert_eq!(
        radarr.unwrap().unwrap().value.lease.unwrap().owner,
        "radarr-worker"
    );
}

#[tokio::test]
async fn rejects_a_save_from_a_worker_that_does_not_own_the_lease() {
    let test = store().await;
    let state = acquire_state(&test.store, ArrKind::Sonarr, "owner", NOW)
        .await
        .unwrap()
        .unwrap();
    let mut tampered = state.clone();
    tampered.value.observations.insert(
        "bad".into(),
        Observation {
            fingerprint: "bad".into(),
            first_seen_at: NOW,
            last_seen_at: NOW,
            observations: 1,
            last_assessed_at: None,
            reason: None,
        },
    );
    assert!(
        save_state(&test.store, &tampered, "intruder", NOW + 1)
            .await
            .is_err()
    );

    let blocked = acquire_state(&test.store, ArrKind::Sonarr, "another-worker", NOW + 2)
        .await
        .unwrap();
    assert!(blocked.is_none());

    release_state(&test.store, ArrKind::Sonarr, "owner", NOW + 2)
        .await
        .unwrap();
    let unchanged = acquire_state(&test.store, ArrKind::Sonarr, "another-worker", NOW + 2)
        .await
        .unwrap()
        .unwrap();
    assert!(unchanged.value.observations.is_empty());
}

#[tokio::test]
async fn fails_closed_when_persisted_nested_action_data_is_malformed() {
    let test = store().await;
    let raw = json!({
        "kind": "radarr",
        "observations": {},
        "actions": [{
            "downloadId": "download-1",
            "title": "Movie",
            "target": {
                "id": "not-a-number", "title": "Movie", "year": 2026, "monitored": true,
                "hasFile": false, "path": "/movies/Movie", "episodeIds": [], "episodes": [],
                "alternateTitles": [],
            },
            "files": [],
            "outputPath": "/downloads/Movie",
            "decision": { "action": "import", "reason": "complete", "source": "rules" },
            "phase": "reserved",
            "createdAt": NOW,
            "updatedAt": NOW,
            "notification": "pending",
        }],
    });
    let value = cbor::to_value(&raw).unwrap();
    let pk = entity::pk::<StoredRecoveryState>(&"radarr".to_owned()).unwrap();
    test.store
        .write(move |tx| -> Result<(), StoreError> {
            tx.upsert_doc(
                &pk,
                &value,
                DocMeta {
                    entity: Some(StoredRecoveryState::NAME.to_owned()),
                    version: 0,
                    expires_at: None,
                    updated_at: Some(NOW),
                },
            )
        })
        .await
        .unwrap();

    assert!(
        acquire_state(&test.store, ArrKind::Radarr, "worker", NOW)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn does_not_let_an_expired_owner_release_a_newer_workers_lease() {
    let test = store().await;
    acquire_state(&test.store, ArrKind::Sonarr, "old-owner", NOW)
        .await
        .unwrap();
    let after_expiry = NOW + RECOVERY_LEASE_MS;
    let current = acquire_state(&test.store, ArrKind::Sonarr, "new-owner", after_expiry)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.value.lease.unwrap().owner, "new-owner");

    release_state(&test.store, ArrKind::Sonarr, "old-owner", after_expiry)
        .await
        .unwrap();
    assert!(
        acquire_state(
            &test.store,
            ArrKind::Sonarr,
            "third-worker",
            after_expiry + 1
        )
        .await
        .unwrap()
        .is_none()
    );
}
