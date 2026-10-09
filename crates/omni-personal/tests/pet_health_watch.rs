//! The pet health watch: durable once-per-week alert delivery, the PetTracker
//! data-gap failure and its single push, and the ERROR-alert gate.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use futures::future::BoxFuture;
use jiff::tz::TimeZone;
use omni_alerts::AlertGate;
use omni_api::pets::PetHealthKind;
use omni_core::clock::{SharedClock, TestClock};
use omni_personal::pets::alerts::{
    GAP_ERROR_PREFIX, HealthAlertError, HealthLedger, HealthNotifier, PetGapAlertGate, PushStatus,
};
use omni_personal::pets::api::WhiskerApi;
use omni_personal::pets::auth::WhiskerAuth;
use omni_personal::pets::health::{Assessment, DAY_MS, HOUR_MS, Notice, Signal};
use omni_personal::pets::persistence::{PetRow, PetStore, WeightHistoryRow};
use omni_personal::pets::task::{PetSyncError, PetTrackerTask};
use omni_personal::reset_alerts::NotifyError;
use omni_store::EntityOps as _;
use omni_store::cbor::Extra;
use omni_store::entity::{EntityWrite as _, UpsertOpts};
use omni_tasks::{Task, TaskRunData, TaskRunStatus, Trigger};
use omni_testkit::{TestStore, mock_http, mock_server, test_clock};
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, ResponseTemplate};

const NOW: i64 = 1_791_000_000_000;

/// Records every push; replies from a script, then `Ok`.
#[derive(Default)]
struct FakeNotifier {
    sent: Mutex<Vec<Notice>>,
    script: Mutex<VecDeque<Result<(), NotifyError>>>,
}

impl FakeNotifier {
    fn calls(&self) -> Vec<Notice> {
        self.sent.lock().unwrap().clone()
    }

    fn fail_next(&self, error: NotifyError) {
        self.script.lock().unwrap().push_back(Err(error));
    }
}

impl HealthNotifier for FakeNotifier {
    fn notify<'a>(&'a self, notice: &'a Notice) -> BoxFuture<'a, Result<(), NotifyError>> {
        self.sent.lock().unwrap().push(notice.clone());
        let reply = self.script.lock().unwrap().pop_front().unwrap_or(Ok(()));
        Box::pin(async move { reply })
    }
}

fn tripped(pet_id: &str, kind: PetHealthKind, value: f64) -> Assessment {
    Assessment {
        pet_id: pet_id.into(),
        kind,
        signal: Signal::Tripped {
            value,
            title: format!("{} {value}", kind.as_str()),
            message: format!("{pet_id} {value}"),
        },
    }
}

async fn ledger() -> (TestStore, Arc<FakeNotifier>, HealthLedger) {
    let clock: Arc<TestClock> = test_clock(NOW);
    let store = TestStore::new(clock).await;
    let notifier = Arc::new(FakeNotifier::default());
    let ledger = HealthLedger::new(
        store.store.clone(),
        Some(notifier.clone() as Arc<dyn HealthNotifier>),
    );
    (store, notifier, ledger)
}

#[tokio::test(start_paused = true)]
async fn repeated_ticks_within_a_week_send_once() {
    let (_store, notifier, ledger) = ledger().await;
    let drop = [tripped("PET-1", PetHealthKind::WeightDrop2w, 3.2)];
    assert_eq!(ledger.apply(&drop, NOW).await.unwrap(), 1);
    for hours in [1, 10, 48, 6 * 24] {
        assert_eq!(ledger.apply(&drop, NOW + hours * HOUR_MS).await.unwrap(), 0);
    }
    assert_eq!(notifier.calls().len(), 1);
    let rows = ledger.all().await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].push, Some(PushStatus::Sent));
    assert!(rows[0].active && rows[0].notified);
    let info = rows[0].info();
    assert_eq!(info.pet_id.as_deref(), Some("PET-1"));
    assert_eq!(info.last_message.as_deref(), Some("PET-1 3.2"));

    // A clear, then a new episode two days later, waits out the week.
    let clear = [Assessment {
        pet_id: "PET-1".into(),
        kind: PetHealthKind::WeightDrop2w,
        signal: Signal::Clear { recovery: None },
    }];
    ledger
        .apply(&clear, NOW + 7 * DAY_MS - HOUR_MS * 30)
        .await
        .unwrap();
    ledger
        .apply(&drop, NOW + 7 * DAY_MS - HOUR_MS * 20)
        .await
        .unwrap();
    assert_eq!(notifier.calls().len(), 1);
    ledger.apply(&drop, NOW + 7 * DAY_MS).await.unwrap();
    assert_eq!(notifier.calls().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn a_definite_rejection_releases_the_reservation_for_the_next_pass() {
    let (_store, notifier, ledger) = ledger().await;
    let visits = [tripped("PET-1", PetHealthKind::VisitDrop, 0.4)];
    notifier.fail_next(NotifyError::Rejected {
        status: 400,
        body: "bad".into(),
    });
    let error = ledger.apply(&visits, NOW).await.unwrap_err();
    assert!(matches!(
        error,
        HealthAlertError::Delivery {
            uncertain: false,
            ..
        }
    ));
    assert!(ledger.all().await.unwrap().is_empty());
    assert_eq!(ledger.apply(&visits, NOW + 10 * 60_000).await.unwrap(), 1);
    assert_eq!(notifier.calls().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn an_uncertain_push_is_never_resent() {
    let (_store, notifier, ledger) = ledger().await;
    let gap = [tripped("*", PetHealthKind::DataGap, 50.0)];
    notifier.fail_next(NotifyError::Uncertain("timeout".into()));
    let error = ledger.apply(&gap, NOW).await.unwrap_err();
    assert!(matches!(
        error,
        HealthAlertError::Delivery {
            uncertain: true,
            ..
        }
    ));
    let row = ledger.gap().await.unwrap().unwrap();
    assert_eq!(row.push, Some(PushStatus::Sending));
    assert_eq!(row.info().pet_id, None);
    assert_eq!(ledger.apply(&gap, NOW + DAY_MS).await.unwrap(), 0);
    assert_eq!(notifier.calls().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn without_pushover_the_state_is_left_for_the_first_configured_run() {
    let (store, _notifier, _ledger) = ledger().await;
    let ledger = HealthLedger::new(store.store.clone(), None);
    let drop = [tripped("PET-1", PetHealthKind::WeightDrop90d, 6.0)];
    assert_eq!(ledger.apply(&drop, NOW).await.unwrap(), 0);
    assert!(ledger.all().await.unwrap().is_empty());
}

fn jwt() -> String {
    let encode = |v: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v);
    format!(
        "h.{}.s",
        encode(
            json!({"mid": "user-1", "exp": 4_102_444_800_u64})
                .to_string()
                .as_bytes()
        )
    )
}

async fn whisker(last_reading: &str) -> wiremock::MockServer {
    let server = mock_server().await;
    Mock::given(method("POST"))
        .and(header("x-amz-target", "AWSCognitoIdentityProviderService.InitiateAuth"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ChallengeName": "PASSWORD_VERIFIER",
            "ChallengeParameters": {"SALT": "ab", "SECRET_BLOCK": "c2VjcmV0", "SRP_B": "0f0e0d", "USER_ID_FOR_SRP": "u"}
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(header(
            "x-amz-target",
            "AWSCognitoIdentityProviderService.RespondToAuthChallenge",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"AuthenticationResult": {"IdToken": jwt()}})),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/graphql/"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"data": {"getPetsByUser": [
                {"petId": "PET-1", "name": "Sam", "weight": 13.4, "lastWeightReading": 13.4,
                 "weightHistory": [{"weight": 13.4, "timestamp": last_reading}]}
            ]}})),
        )
        .mount(&server)
        .await;
    server
}

async fn insert_run(store: &omni_store::Store, minutes: i64, error: &str) {
    let run = TaskRunData {
        run_id: format!("PetTracker:{minutes}"),
        task_name: "PetTracker".into(),
        trigger: Trigger::Schedule,
        scheduled_for: None,
        started_at: NOW + minutes * 60_000,
        finished_at: Some(NOW + minutes * 60_000),
        status: TaskRunStatus::Error,
        error: Some(error.into()),
        summary: None,
        extra: Extra::default(),
    };
    store
        .write(move |tx| tx.upsert(&run, UpsertOpts::default()))
        .await
        .unwrap();
}

// Real time: wiremock needs it, and the test clock then drifts only by milliseconds.
#[tokio::test]
async fn a_48_hour_silence_fails_the_run_and_pushes_once() {
    // The last reading is 72 hours old, stamped like Whisker (UTC, no offset).
    let last_iso = omni_core::js::to_iso_string(NOW - 72 * HOUR_MS);
    let last = last_iso.trim_end_matches(".000Z");
    let server = whisker(last).await;
    let clock: Arc<TestClock> = test_clock(NOW);
    let shared: SharedClock = clock.clone();
    let store = TestStore::new(shared.clone()).await;
    let pets = PetStore::open(&store.store).await.unwrap();
    pets.upsert_pet(&PetRow {
        pet_id: "PET-1".into(),
        name: "Sam".into(),
        current_weight: 13.4,
        updated_at: "2026-09-30T13:30:00.000Z".into(),
    })
    .await
    .unwrap();
    pets.insert_weight_reading(&WeightHistoryRow {
        pet_id: "PET-1".into(),
        timestamp: last.into(),
        weight: 13.4,
    })
    .await
    .unwrap();
    let http = mock_http(
        &server,
        &[
            "https://cognito-idp.us-east-1.amazonaws.com",
            "https://pet-profile.iothings.site",
        ],
    );
    let notifier = Arc::new(FakeNotifier::default());
    let ledger = HealthLedger::new(
        store.store.clone(),
        Some(notifier.clone() as Arc<dyn HealthNotifier>),
    );
    let task = PetTrackerTask::new(
        Arc::new(WhiskerAuth::new(
            http.clone(),
            shared.clone(),
            "e".into(),
            "p".into(),
        )),
        WhiskerApi::new(http),
        pets,
        ledger.clone(),
        shared,
        TimeZone::UTC,
    )
    .unwrap();

    let error = task.run_pass().await.unwrap_err();
    assert!(matches!(error, PetSyncError::DataGap { .. }), "{error}");
    let message = error.to_string();
    assert!(message.starts_with(GAP_ERROR_PREFIX), "{message}");
    assert!(message.contains("72 h"), "{message}");
    let summary = task.last_run_summary().unwrap();
    assert!(
        summary.starts_with("No litter-box readings for any pet since "),
        "{summary}"
    );
    assert!(
        summary.ends_with("0 new readings, 1 health alert sent"),
        "{summary}"
    );
    let calls = notifier.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].title(), "Pet scale: no readings for 48 h");

    clock.set(NOW + HOUR_MS);
    assert!(matches!(
        task.run_pass().await.unwrap_err(),
        PetSyncError::DataGap { .. }
    ));
    assert_eq!(notifier.calls().len(), 1);

    // The run failure stays off the ERROR-alert path while the watch owns the gap.
    let gate = PetGapAlertGate::new(store.store.clone());
    let title = "Error running task \"PetTracker\"";
    insert_run(&store.store, 1, &message).await;
    assert!(!gate.should_notify(title).await);
    // Any other failure still alerts.
    insert_run(&store.store, 2, "Whisker authentication failed").await;
    assert!(gate.should_notify(title).await);
    let runs = store
        .store
        .read(|docs| docs.get_all::<TaskRunData>())
        .await
        .unwrap();
    assert_eq!(runs.len(), 2);
}

#[tokio::test(start_paused = true)]
async fn the_gate_admits_non_gap_failures() {
    let store = TestStore::new(test_clock(NOW)).await;
    let gate = PetGapAlertGate::new(store.store.clone());
    let title = "Manual run of \"PetTracker\" failed";
    assert!(gate.applies(title));
    assert!(gate.should_notify(title).await);
    insert_run(&store.store, 1, "Whisker API returned status code 500").await;
    assert!(gate.should_notify(title).await);
    // A gap error without a tracked gap row (the ledger never recorded it) alerts.
    insert_run(
        &store.store,
        2,
        &format!("{GAP_ERROR_PREFIX} for 50 h (latest x)"),
    )
    .await;
    assert!(gate.should_notify(title).await);
}
