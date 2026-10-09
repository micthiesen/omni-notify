//! Rows written by node-cbor in the exact shape the former TS code wrote them
//! (committed `tests/golden/ts_rows.json`). The production copy has no actions, scopes,
//! papercuts or notifications yet, so these cover their decoders.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_store::cbor::{self, JsValue};
use omni_workspaces::entities::{
    ActionRow, EmailScopeRow, NotificationRow, NotificationStatus, PapercutRow, SourceRow,
};
use serde::Serialize;
use serde::de::DeserializeOwned;

fn bytes(name: &str) -> Vec<u8> {
    let hex = omni_testkit::golden("ts_rows.json")[name]
        .as_str()
        .unwrap()
        .to_owned();
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect()
}

fn strip_undefined(value: JsValue) -> JsValue {
    match value {
        JsValue::Object(map) => JsValue::Object(
            map.into_iter()
                .filter(|(_, v)| *v != JsValue::Undefined)
                .map(|(k, v)| (k, strip_undefined(v)))
                .collect(),
        ),
        JsValue::Array(items) => JsValue::Array(items.into_iter().map(strip_undefined).collect()),
        other => other,
    }
}

/// Decodes the node bytes, re-encodes, and returns (typed, byte-identical).
fn round_trip<T: Serialize + DeserializeOwned>(name: &str) -> (T, bool) {
    let original = bytes(name);
    let value = cbor::decode(&original).unwrap();
    let typed: T = cbor::from_value(value.clone()).unwrap();
    let encoded = cbor::encode(&cbor::to_value(&typed).unwrap());
    assert_eq!(
        cbor::decode(&encoded).unwrap(),
        strip_undefined(value),
        "{name}"
    );
    (typed, encoded == original)
}

#[test]
fn actions_round_trip_byte_for_byte() {
    let (action, identical) = round_trip::<ActionRow>("workspace-action");
    assert!(identical, "engine key order matches the struct");
    assert_eq!(
        action.payload,
        r#"{"title":"Return","startDate":"2026-09-01","allDay":true,"reminderMinutes":1440}"#
    );
    let (resolved, _) = round_trip::<ActionRow>("workspace-action-resolved");
    assert_eq!(resolved.run_id, None);
    assert_eq!(resolved.resolved_at, Some(2));
}

#[test]
fn scopes_papercuts_notifications_and_email_sources_decode() {
    let (scope, identical) = round_trip::<EmailScopeRow>("workspace-email-scope");
    assert!(identical);
    assert_eq!(scope.subject_keywords, ["lens"]);
    let (papercut, _) = round_trip::<PapercutRow>("workspace-papercut");
    assert_eq!(papercut.subject_id, None);
    assert_eq!(papercut.occurrences, 2);
    let (notification, _) = round_trip::<NotificationRow>("workspace-notification");
    assert_eq!(notification.status, NotificationStatus::Sent);
    assert_eq!(notification.sent_at, Some(7));
    let (source, _) = round_trip::<SourceRow>("workspace-source-email");
    assert_eq!(source.triggered_at, Some(9));
    assert_eq!(source.email_id.as_deref(), Some("<m@x>"));
}
