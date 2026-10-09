//! Port of `src/pet-tracker/api.spec.ts` (all cases kept), plus GraphQL error
//! handling and the Cognito SRP sign-in against stubbed endpoints.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use base64::Engine as _;
use omni_core::clock::SharedClock;
use omni_personal::pets::api::{WeightReading, WhiskerApi, WhiskerPet};
use omni_personal::pets::auth::WhiskerAuth;
use omni_testkit::{TEST_EPOCH_MS, mock_http, mock_server, test_clock};
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, ResponseTemplate};

#[tokio::test]
async fn treats_a_null_weight_history_as_no_recent_readings() {
    let server = mock_server().await;
    Mock::given(method("POST"))
        .and(path("/graphql/"))
        .and(header("authorization", "Bearer token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {"getPetsByUser": [{
                "petId": "pet-1", "name": "Sam", "weight": 12.6,
                "lastWeightReading": 12.6, "weightHistory": null
            }]}
        })))
        .mount(&server)
        .await;
    let api = WhiskerApi::new(mock_http(&server, &["https://pet-profile.iothings.site"]));
    let pets = api.fetch_pets_by_user("token", "user").await.unwrap();
    assert_eq!(
        pets,
        vec![WhiskerPet {
            pet_id: "pet-1".into(),
            name: "Sam".into(),
            weight: 12.6,
            last_weight_reading: 12.6,
            weight_history: vec![],
        }]
    );
    let requests = server.received_requests().await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body["variables"], json!({"userId": "user"}));
}

#[tokio::test]
async fn reports_graphql_errors_and_missing_data() {
    let server = mock_server().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "errors": [{"message": "a"}, {"message": "b"}]
        })))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&server)
        .await;
    let api = WhiskerApi::new(mock_http(&server, &["https://pet-profile.iothings.site"]));
    let error = api
        .fetch_weight_history("t", "pet-1", Some(5))
        .await
        .unwrap_err();
    assert_eq!(error.operation, "GetWeightHistoryByPetId");
    assert_eq!(error.cause, "GraphQL errors: a; b");
    let error = api.fetch_pets_by_user("t", "u").await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "GetPetsByUser failed: GraphQL response missing data"
    );
}

#[tokio::test]
async fn reads_weight_history_with_an_optional_limit() {
    let server = mock_server().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {"getWeightHistoryByPetId": [{"weight": 12.1, "timestamp": "2026-03-20T15:37:39"}]}
        })))
        .mount(&server)
        .await;
    let api = WhiskerApi::new(mock_http(&server, &["https://pet-profile.iothings.site"]));
    let history = api.fetch_weight_history("t", "pet-1", None).await.unwrap();
    assert_eq!(
        history,
        vec![WeightReading {
            weight: 12.1,
            timestamp: "2026-03-20T15:37:39".into()
        }]
    );
    let requests = server.received_requests().await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body["variables"], json!({"petId": "pet-1"}));
}

fn jwt(payload: serde_json::Value) -> String {
    let encode = |v: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v);
    format!(
        "{}.{}.sig",
        encode(br#"{"alg":"none"}"#),
        encode(payload.to_string().as_bytes())
    )
}

#[tokio::test]
async fn signs_in_with_srp_and_caches_the_id_token_until_near_expiry() {
    let server = mock_server().await;
    Mock::given(method("POST"))
        .and(header(
            "x-amz-target",
            "AWSCognitoIdentityProviderService.InitiateAuth",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ChallengeName": "PASSWORD_VERIFIER",
            "ChallengeParameters": {
                "SALT": "a1b2c3d4",
                "SECRET_BLOCK": "c2VjcmV0",
                "SRP_B": "0f0e0d0c0b0a09080706050403020100",
                "USERNAME": "user-uuid",
                "USER_ID_FOR_SRP": "user-uuid"
            }
        })))
        .mount(&server)
        .await;
    let exp = (TEST_EPOCH_MS / 1000) + 3600;
    Mock::given(method("POST"))
        .and(header(
            "x-amz-target",
            "AWSCognitoIdentityProviderService.RespondToAuthChallenge",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "AuthenticationResult": {"IdToken": jwt(json!({"mid": "user-123", "exp": exp}))}
        })))
        .mount(&server)
        .await;
    let clock = test_clock(TEST_EPOCH_MS);
    let shared: SharedClock = clock.clone();
    let auth = WhiskerAuth::new(
        mock_http(&server, &["https://cognito-idp.us-east-1.amazonaws.com"]),
        shared,
        "me@example.com".into(),
        "password".into(),
    );
    let session = auth.authenticate().await.unwrap();
    assert_eq!(session.user_id, "user-123");
    let again = auth.authenticate().await.unwrap();
    assert_eq!(again, session);
    assert_eq!(server.received_requests().await.unwrap().len(), 2);

    // Within five minutes of expiry the token is refreshed.
    clock.set(TEST_EPOCH_MS + 56 * 60 * 1000);
    auth.authenticate().await.unwrap();
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 4);
    let respond: serde_json::Value = serde_json::from_slice(&requests[1].body).unwrap();
    assert_eq!(
        respond["ChallengeResponses"]["USERNAME"],
        json!("user-uuid")
    );
    assert_eq!(
        respond["ChallengeResponses"]["PASSWORD_CLAIM_SECRET_BLOCK"],
        json!("c2VjcmV0")
    );
}

#[tokio::test]
async fn surfaces_cognito_rejections() {
    let server = mock_server().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "__type": "NotAuthorizedException", "message": "Incorrect username or password."
        })))
        .mount(&server)
        .await;
    let auth = WhiskerAuth::new(
        mock_http(&server, &["https://cognito-idp.us-east-1.amazonaws.com"]),
        test_clock(TEST_EPOCH_MS),
        "me@example.com".into(),
        "wrong".into(),
    );
    let error = auth.authenticate().await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Incorrect username or password."),
        "{error}"
    );
}
