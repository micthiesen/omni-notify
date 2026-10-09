//! Port of `src/pet-tracker/task.spec.ts` (all cases kept), plus a full sync
//! against stubbed Cognito and Whisker endpoints and the log line format.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use base64::Engine as _;
use jiff::tz::TimeZone;
use omni_core::clock::SharedClock;
use omni_personal::pets::api::WhiskerApi;
use omni_personal::pets::auth::WhiskerAuth;
use omni_personal::pets::persistence::PetStore;
use omni_personal::pets::task::{PetTrackerTask, pet_display_name, round_fixed};
use omni_tasks::Task;
use omni_testkit::{TestStore, capture_logs, mock_http, mock_server, test_clock};
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, ResponseTemplate};

#[test]
fn swaps_whiskers_reversed_sam_and_sandy_profiles() {
    assert_eq!(
        pet_display_name("PET-b4738d2e-9a37-4d70-b401-a86e56bfd180", "Sam"),
        "Sandy"
    );
    assert_eq!(
        pet_display_name("PET-697f1644-6b4b-43cb-945b-61426edcbb86", "Sandy"),
        "Sam"
    );
}

#[test]
fn keeps_whiskers_name_for_other_pets() {
    assert_eq!(pet_display_name("PET-other", "Luna"), "Luna");
}

#[test]
fn rounds_like_math_round_then_to_fixed() {
    assert_eq!(round_fixed(12.617_988, 1), "12.6");
    assert_eq!(round_fixed(0.125, 2), "0.13");
    assert_eq!(round_fixed(-0.001, 2), "0.00");
    assert_eq!(round_fixed(-0.126, 2), "-0.13");
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

#[tokio::test]
async fn syncs_pets_and_new_readings_idempotently() {
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
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": {"getPetsByUser": [
            {"petId": "PET-b4738d2e-9a37-4d70-b401-a86e56bfd180", "name": "Sam", "weight": 12.61,
             "lastWeightReading": 12.6, "weightHistory": [
                {"weight": 12.5, "timestamp": "2026-10-01T10:00:00"},
                {"weight": 12.6, "timestamp": "2026-10-08T10:00:00"}
             ]},
            {"petId": "PET-x", "name": "Luna", "weight": 9.0, "lastWeightReading": 9.0, "weightHistory": null}
        ]}})))
        .mount(&server)
        .await;
    let now = "2026-10-09T12:00:00Z"
        .parse::<jiff::Timestamp>()
        .unwrap()
        .as_millisecond();
    let clock: SharedClock = test_clock(now);
    let store = TestStore::new(clock.clone()).await;
    let http = mock_http(
        &server,
        &[
            "https://cognito-idp.us-east-1.amazonaws.com",
            "https://pet-profile.iothings.site",
        ],
    );
    let pets = PetStore::open(&store.store).await.unwrap();
    let task = PetTrackerTask::new(
        Arc::new(WhiskerAuth::new(
            http.clone(),
            clock.clone(),
            "e".into(),
            "p".into(),
        )),
        WhiskerApi::new(http),
        pets.clone(),
        clock,
        TimeZone::get("America/Vancouver").unwrap(),
    )
    .unwrap();
    assert_eq!(task.name(), "PetTracker");
    assert_eq!(task.schedule().as_str(), "0 */10 * * * *");
    assert!(task.options().run_on_startup);

    let logs = capture_logs();
    task.sync().await.unwrap();
    let all = pets.all_pets().await.unwrap();
    assert_eq!(all.len(), 2);
    let sandy = pets
        .get_pet("PET-b4738d2e-9a37-4d70-b401-a86e56bfd180")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(sandy.name, "Sandy");
    assert_eq!(sandy.current_weight, 12.61);
    // The test clock follows real elapsed time, so only the second is fixed.
    assert!(
        sandy.updated_at.starts_with("2026-10-09T12:00:0"),
        "{}",
        sandy.updated_at
    );
    assert_eq!(pets.weight_history(&sandy.pet_id).await.unwrap().len(), 2);
    let messages: Vec<String> = logs.events().into_iter().map(|e| e.message).collect();
    assert!(
        messages
            .iter()
            .any(|m| m == "Synced 2 pets, 2 new / 2 total readings"),
        "{messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|m| m.starts_with("Sandy: 12.6 lbs (+0.10 lbs/wk")),
        "{messages:?}"
    );

    // A second pass inserts nothing new.
    task.sync().await.unwrap();
    assert_eq!(pets.weight_history(&sandy.pet_id).await.unwrap().len(), 2);
}
