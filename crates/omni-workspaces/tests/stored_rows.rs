//! Rows encoded by node-cbor in the exact stored shape
//! (committed `tests/golden/stored_rows.json`). The production copy has no actions, scopes,
//! papercuts or notifications yet, so these cover their decoders.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_store::cbor;
use omni_workspaces::entities::{
    ActionRow, EmailScopeRow, NotificationRow, NotificationStatus, PapercutRow, SourceRow,
};
use serde::Serialize;
use serde::de::DeserializeOwned;

fn bytes(name: &str) -> Vec<u8> {
    let hex = omni_testkit::golden("stored_rows.json")[name]
        .as_str()
        .unwrap()
        .to_owned();
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect()
}

/// Decodes the node bytes and checks the typed re-encoding reads back as the
/// same value.
fn round_trip<T: Serialize + DeserializeOwned>(name: &str) -> T {
    let value = cbor::decode(&bytes(name)).unwrap();
    let typed: T = cbor::from_value(value.clone()).unwrap();
    let encoded = cbor::encode(&cbor::to_value(&typed).unwrap());
    assert!(
        cbor::same_value(&cbor::decode(&encoded).unwrap(), &value),
        "{name}"
    );
    typed
}

#[test]
fn actions_round_trip() {
    let action = round_trip::<ActionRow>("workspace-action");
    assert_eq!(
        action.payload,
        r#"{"title":"Return","startDate":"2026-09-01","allDay":true,"reminderMinutes":1440}"#
    );
    let resolved = round_trip::<ActionRow>("workspace-action-resolved");
    assert_eq!(resolved.run_id, None);
    assert_eq!(resolved.resolved_at, Some(2));
}

#[test]
fn scopes_papercuts_notifications_and_email_sources_decode() {
    let scope = round_trip::<EmailScopeRow>("workspace-email-scope");
    assert_eq!(scope.subject_keywords, ["lens"]);
    let papercut = round_trip::<PapercutRow>("workspace-papercut");
    assert_eq!(papercut.subject_id, None);
    assert_eq!(papercut.occurrences, 2);
    let notification = round_trip::<NotificationRow>("workspace-notification");
    assert_eq!(notification.status, NotificationStatus::Sent);
    assert_eq!(notification.sent_at, Some(7));
    let source = round_trip::<SourceRow>("workspace-source-email");
    assert_eq!(source.triggered_at, Some(9));
    assert_eq!(source.email_id.as_deref(), Some("<m@x>"));
}
