//! `HttpError::is_transient`: transport failures, rate limits and server errors
//! are retryable; client failures and programming or policy errors are not.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_http::HttpError;

fn status(status: u16) -> HttpError {
    HttpError::Status {
        status,
        body: String::new(),
    }
}

#[test]
fn retries_rate_limits_and_server_failures() {
    for code in [429, 500, 503] {
        assert!(status(code).is_transient(), "{code}");
    }
}

#[test]
fn does_not_retry_client_failures() {
    for code in [400, 401, 404, 422] {
        assert!(!status(code).is_transient(), "{code}");
    }
}

#[test]
fn retries_transport_failures() {
    assert!(HttpError::Network("connection reset".to_owned()).is_transient());
    assert!(HttpError::Timeout.is_transient());
}

#[test]
fn does_not_retry_programming_or_policy_errors() {
    assert!(!HttpError::InvalidUrl("bad".to_owned()).is_transient());
    assert!(!HttpError::Decode("bad json".to_owned()).is_transient());
    assert!(!HttpError::Blocked("private".to_owned()).is_transient());
    assert!(!HttpError::TooLarge { limit: 1 }.is_transient());
}
