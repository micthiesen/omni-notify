//! Shared fixtures for the CloudKit, recurrence and service tests.
#![allow(
    dead_code,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::type_complexity
)]

use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use omni_reminders::cloudkit::{CkPost, RemindersCloudKitClient, RemindersError};
use omni_reminders::codec::encode_crdt_document;
use serde_json::{Value, json};

pub const NOW: i64 = 1_800_000_000_000;

/// A synchronous `(path, body) -> response` CloudKit fake.
pub fn post(
    f: impl Fn(&str, Value) -> Result<Value, RemindersError> + Send + Sync + 'static,
) -> Arc<dyn CkPost> {
    Arc::new(
        move |path: &str, body: Value| -> BoxFuture<'static, Result<Value, RemindersError>> {
            let result = f(path, body);
            Box::pin(async move { result })
        },
    )
}

pub fn client(
    f: impl Fn(&str, Value) -> Result<Value, RemindersError> + Send + Sync + 'static,
) -> RemindersCloudKitClient {
    RemindersCloudKitClient::new(post(f), omni_testkit::test_clock(NOW))
}

/// Records `(path, body)` pairs.
#[derive(Clone, Default)]
pub struct Calls(pub Arc<Mutex<Vec<(String, Value)>>>);

impl Calls {
    pub fn push(&self, path: &str, body: &Value) {
        self.0.lock().unwrap().push((path.to_owned(), body.clone()));
    }

    pub fn paths(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(|(p, _)| p.clone())
            .collect()
    }

    pub fn bodies(&self) -> Vec<Value> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(|(_, b)| b.clone())
            .collect()
    }

    pub fn len(&self) -> usize {
        self.0.lock().unwrap().len()
    }
}

pub fn doc(text: &str) -> Value {
    json!({"type": "STRING", "value": encode_crdt_document(text).unwrap()})
}

/// The spec's `record()` fixture.
pub fn record() -> Value {
    json!({
        "recordName": "Reminder/12345678",
        "recordType": "Reminder",
        "recordChangeTag": "tag-1",
        "fields": {
            "TitleDocument": doc("Buy milk"),
            "NotesDocument": doc("At noon"),
            "List": {"type": "REFERENCE", "value": {"recordName": "List/1", "action": "VALIDATE"}},
            "Completed": {"type": "INT64", "value": 0},
            "DueDate": {"type": "TIMESTAMP", "value": 1_699_920_000_000_i64},
            "Priority": {"type": "INT64", "value": 5},
            "Flagged": {"type": "INT64", "value": 1},
            "AllDay": {"type": "INT64", "value": 1},
        },
    })
}

/// `record({...overrides})`.
pub fn record_with(overrides: Value) -> Value {
    let mut base = record();
    for (k, v) in overrides.as_object().unwrap() {
        base[k] = v.clone();
    }
    base
}

/// `record({fields: {...record().fields, ...fields}})`.
pub fn record_fields(fields: Value) -> Value {
    let mut base = record();
    for (k, v) in fields.as_object().unwrap() {
        base["fields"][k] = v.clone();
    }
    base
}

pub fn list() -> Value {
    json!({
        "recordName": "List/1",
        "recordType": "List",
        "fields": {
            "Name": {"type": "STRING", "value": "Groceries"},
            "Color": {"type": "STRING", "value": "blue"},
            "Count": {"type": "INT64", "value": 1},
        },
    })
}

pub fn rule_record(name: &str, reminder: &str) -> Value {
    json!({
        "recordName": name,
        "recordType": "RecurrenceRule",
        "fields": {"Reminder": {"type": "REFERENCE", "value": {"recordName": reminder}}},
    })
}

pub fn zones(records: Value, extra: Value) -> Value {
    let mut zone = json!({"records": records});
    for (k, v) in extra.as_object().unwrap() {
        zone[k] = v.clone();
    }
    json!({"zones": [zone]})
}

pub fn sync_token(body: &Value) -> Value {
    body["zones"][0]
        .get("syncToken")
        .cloned()
        .unwrap_or(Value::Null)
}

pub fn list_of_query(body: &Value) -> String {
    body["query"]["filterBy"][0]["fieldValue"]["value"]["recordName"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}
