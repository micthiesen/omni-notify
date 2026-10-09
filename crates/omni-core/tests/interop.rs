//! Port of `src/effect/interop.spec.ts`. `fromPromise` / `fromSync` become a
//! leaf `map_err` into `IntegrationError` / `PersistenceError`; the spec checks
//! the error's tag and its `"<operation> failed: <cause>"` message.

use omni_core::error::{IntegrationError, PersistenceError};

fn fetch_widget() -> Result<(), std::io::Error> {
    Err(std::io::Error::other("offline"))
}

#[test]
fn maps_rejected_promises_to_an_integration_error() {
    let error = fetch_widget()
        .map_err(|e| IntegrationError::new("fetch widget", e))
        .expect_err("rejected");
    assert!(format!("{error:?}").contains("IntegrationError"));
    assert_eq!(error.to_string(), "fetch widget failed: offline");
}

#[test]
fn maps_thrown_persistence_failures_to_a_persistence_error() {
    let error = Err::<(), _>("disk full")
        .map_err(|e| PersistenceError::new("save widget", e))
        .expect_err("thrown");
    assert!(format!("{error:?}").contains("PersistenceError"));
    assert_eq!(error.to_string(), "save widget failed: disk full");
}
