#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_arguments
)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use omni_core::clock::{SharedClock, TestClock};
use omni_mcp_kit::{McpTool, ToolContext, ToolOutput};
use omni_stylist::Stylist;
use omni_testkit::TestStore;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

/// 2026-10-12T18:00:00Z.
pub const NOW_MS: i64 = 1_791_828_000_000;

pub struct Harness {
    pub stylist: Stylist,
    pub clock: Arc<TestClock>,
    pub private: PathBuf,
    tools: HashMap<String, McpTool>,
    store: Arc<TestStore>,
    dir: Arc<tempfile::TempDir>,
}

/// A tool's structured result plus any embedded resources.
pub struct Called {
    pub structured: Value,
    pub resources: Vec<Value>,
}

fn tools_for(stylist: &Stylist) -> HashMap<String, McpTool> {
    omni_stylist::mcp::tools(Arc::new(stylist.clone()))
        .unwrap()
        .into_iter()
        .map(|tool| (tool.meta.name.clone(), tool))
        .collect()
}

impl Harness {
    pub async fn new() -> Self {
        let clock = TestClock::new(NOW_MS);
        let shared: SharedClock = clock.clone();
        let store = Arc::new(TestStore::new(shared.clone()).await);
        let dir = Arc::new(tempfile::tempdir().unwrap());
        let private = dir.path().join("stylist-private");
        let stylist = Stylist::new(
            store.store.clone(),
            shared,
            jiff::tz::TimeZone::UTC,
            private.clone(),
        );
        Self {
            tools: tools_for(&stylist),
            stylist,
            clock,
            private,
            store,
            dir,
        }
    }

    /// A new, empty database with an existing private directory (database loss).
    pub async fn with_private(private: PathBuf) -> Self {
        let clock = TestClock::new(NOW_MS);
        let shared: SharedClock = clock.clone();
        let store = Arc::new(TestStore::new(shared.clone()).await);
        let dir = Arc::new(tempfile::tempdir().unwrap());
        let stylist = Stylist::new(
            store.store.clone(),
            shared,
            jiff::tz::TimeZone::UTC,
            private.clone(),
        );
        Self {
            tools: tools_for(&stylist),
            stylist,
            clock,
            private,
            store,
            dir,
        }
    }

    /// A fresh service over the same database and directory: nothing carried
    /// over in memory, as after a restart or in a new chat session.
    pub fn cold(&self) -> Harness {
        let shared: SharedClock = self.clock.clone();
        let stylist = Stylist::new(
            self.store.store.clone(),
            shared,
            jiff::tz::TimeZone::UTC,
            self.private.clone(),
        );
        Harness {
            tools: tools_for(&stylist),
            stylist,
            clock: self.clock.clone(),
            private: self.private.clone(),
            store: self.store.clone(),
            dir: self.dir.clone(),
        }
    }

    pub fn db_path(&self) -> PathBuf {
        self.store.store.path().to_path_buf()
    }

    pub async fn try_call(&self, name: &str, input: Value) -> Result<Called, String> {
        let tool = self
            .tools
            .get(name)
            .unwrap_or_else(|| panic!("no tool {name}"));
        let cx = ToolContext {
            call_id: "test".to_owned(),
            cancel: CancellationToken::new(),
        };
        match tool.handler.call(input, cx).await {
            Ok(ToolOutput::Structured(map)) => Ok(Called {
                structured: Value::Object(map),
                resources: Vec::new(),
            }),
            Ok(ToolOutput::Custom {
                structured,
                content,
            }) => Ok(Called {
                structured: Value::Object(structured),
                resources: content
                    .into_iter()
                    .filter(|c| c.get("type") == Some(&json!("resource")))
                    .map(|c| c["resource"].clone())
                    .collect(),
            }),
            Err(error) => Err(error.message),
        }
    }

    pub async fn call(&self, name: &str, input: Value) -> Value {
        match self.try_call(name, input.clone()).await {
            Ok(called) => called.structured,
            Err(error) => panic!("{name} failed: {error}\ninput: {input:#}"),
        }
    }

    /// Calls `name` and returns the error text, failing if it succeeds.
    pub async fn reject(&self, name: &str, input: Value) -> String {
        match self.try_call(name, input).await {
            Ok(called) => panic!("{name} unexpectedly succeeded: {:#}", called.structured),
            Err(error) => error,
        }
    }

    pub async fn save(&self, namespace: &str, key: &str, record: Value) -> Value {
        self.call(
            "stylist_record_save",
            json!({"namespace": namespace, "idempotencyKey": key, "record": record}),
        )
        .await
    }

    pub async fn event(
        &self,
        namespace: &str,
        key: &str,
        occurred_on: &str,
        event: Value,
    ) -> Value {
        self.call(
            "stylist_event_append",
            json!({"namespace": namespace, "idempotencyKey": key, "occurredOn": occurred_on, "event": event}),
        )
        .await
    }

    /// The verified bytes of an evidence item, from the embedded resource.
    pub async fn evidence_bytes(&self, namespace: &str, evidence_id: &str) -> Vec<u8> {
        use base64::Engine as _;
        let called = self
            .try_call(
                "stylist_evidence_get",
                json!({"namespace": namespace, "evidenceId": evidence_id}),
            )
            .await
            .unwrap();
        assert_eq!(called.structured["verified"], json!(true));
        assert_eq!(called.structured["blob"], Value::Null);
        let blob = called.resources[0]["blob"].as_str().unwrap();
        base64::engine::general_purpose::STANDARD
            .decode(blob)
            .unwrap()
    }

    pub async fn context(&self, namespace: &str, focus: Value) -> Value {
        self.call(
            "stylist_context_get",
            json!({"namespace": namespace, "focus": focus, "includeRunbook": false}),
        )
        .await
    }
}

/// An explicit measured provenance block.
pub fn measured() -> Value {
    json!({"certainty": "explicit", "source": "user_measurement"})
}

pub fn stated() -> Value {
    json!({"certainty": "explicit", "source": "user_statement"})
}

/// A minimal JPEG-signature byte string.
pub fn jpeg(seed: u8) -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, b'J', b'F', b'I', b'F'];
    bytes.extend(std::iter::repeat_n(seed, 64));
    bytes
}

/// The id in a save, append or store result.
pub fn id(value: &Value) -> String {
    value["record"]["id"]
        .as_str()
        .or_else(|| value["event"]["id"].as_str())
        .or_else(|| value["evidence"]["id"].as_str())
        .unwrap_or_else(|| panic!("no id in {value:#}"))
        .to_owned()
}

/// A key of the required length.
pub fn key(label: &str) -> String {
    format!("test-key-{label}-00000000")
}

pub fn tee(brand: &str, model: &str, size: &str, cut: &str) -> Value {
    json!({
        "kind": "variant",
        "brand": brand,
        "model": model,
        "category": "tops/tshirt",
        "sizeLabel": size,
        "sizeSystem": "alpha",
        "cut": cut,
        "fabric": {"composition": [{"fiber": "cotton", "percent": 100}], "construction": "knit"},
        "provenance": {"certainty": "explicit", "source": "label"},
    })
}

pub fn garment_measurement(
    subject_type: &str,
    subject: &str,
    dimension: &str,
    method: &str,
    value: f64,
    unit: &str,
    wash: &str,
    observed: &str,
) -> Value {
    json!({
        "kind": "measurement",
        "subject": {"type": subject_type, "id": subject},
        "dimension": dimension,
        "method": method,
        "endpoints": format!("{dimension} per runbook, laid flat"),
        "value": value,
        "unit": unit,
        "garmentState": {"stretch": "relaxed", "wash": wash},
        "observedOn": observed,
        "provenance": measured(),
    })
}

pub fn body_measurement(dimension: &str, value: f64, unit: &str, observed: &str) -> Value {
    json!({
        "kind": "measurement",
        "subject": {"type": "body"},
        "dimension": dimension,
        "method": "circumference",
        "endpoints": format!("{dimension} with a soft tape, snug"),
        "value": value,
        "unit": unit,
        "observedOn": observed,
        "provenance": measured(),
    })
}
