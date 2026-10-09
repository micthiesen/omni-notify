//! Castro authentication.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use omni_podcasts::castro::auth::{
    CastroCredentials, CastroRequestToSign, create_castro_auth_headers,
    create_castro_canonical_string, hash_castro_request_body, sign_castro_canonical_string,
};

#[test]
fn hashes_an_empty_request_body_like_the_captured_castro_client() {
    assert_eq!(
        hash_castro_request_body(""),
        "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU="
    );
}

#[test]
fn builds_the_apiauth_canonical_string() {
    let canonical = create_castro_canonical_string(
        &CastroRequestToSign {
            method: "post",
            path_and_query: "/ctrl_api/v1/json",
            date: "Thu, 25 Aug 2022 04:27:52 GMT",
            ..CastroRequestToSign::default()
        },
        "OniJqRAkzQHN8KgmAZm/yT5dP94m8CmVVaSTRVg/ptQ=",
    );
    assert_eq!(
        canonical,
        "POST,application/json,OniJqRAkzQHN8KgmAZm/yT5dP94m8CmVVaSTRVg/ptQ=,/ctrl_api/v1/json,Thu, 25 Aug 2022 04:27:52 GMT"
    );
}

#[test]
fn signs_the_public_apiauth_hmac_sha256_test_vector() {
    let canonical = "POST,application/json,OniJqRAkzQHN8KgmAZm/yT5dP94m8CmVVaSTRVg/ptQ=,/ctrl_api/v1/json,Thu, 25 Aug 2022 04:27:52 GMT";
    let secret = STANDARD
        .decode("AGnO/VenzHB9xkLYZG1i70kQ9iyFBBvugGXSFyTQaB0=")
        .unwrap();
    assert_eq!(
        sign_castro_canonical_string(canonical, &secret),
        "vPI9MMRwBZLWNrCcnLnbJjZRna0+XP7yFMhc9KMUFdw="
    );
}

#[test]
fn returns_all_headers_required_by_castro() {
    let headers = create_castro_auth_headers(
        &CastroCredentials {
            access_id: "device-id".to_owned(),
            secret: b"secret".to_vec(),
        },
        &CastroRequestToSign {
            method: "GET",
            path_and_query: "/ping",
            date: "Thu, 16 Jul 2026 17:26:59 GMT",
            ..CastroRequestToSign::default()
        },
    );
    let signature = headers
        .authorization
        .strip_prefix("APIAuth-HMAC-SHA256 device-id:")
        .unwrap();
    assert_eq!(signature.len(), 44);
    assert!(signature.ends_with('='));
    assert!(
        signature[..43]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/')
    );
    assert_eq!(headers.content_type, "application/json");
    assert_eq!(headers.date, "Thu, 16 Jul 2026 17:26:59 GMT");
    assert_eq!(
        headers.content_sha256,
        "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU="
    );
}
