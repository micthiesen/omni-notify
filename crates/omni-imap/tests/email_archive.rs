//! Email archive receipts and the queue/cancel/status tool flow.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_core::clock::{SharedClock, TestClock};
use omni_imap::archive_service::ArchiveService;
use omni_imap::archive_store::{ArchiveAction, ArchiveActionStatus};
use omni_imap::compose::ComposeService;
use omni_imap::mcp_tools::{ToolDeps, email_tools, serialize_action};
use omni_imap::ops::archive::{ArchiveIdentity, ArchiveLocation};
use omni_mcp_kit::{ToolContext, ToolOutput};
use omni_store::cbor::Extra;
use omni_testkit::TestStore;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

#[test]
fn returns_the_exact_new_inbox_identity_after_restore() {
    let action = ArchiveAction {
        action_id: "a".repeat(64),
        identity: ArchiveIdentity {
            folder: "INBOX".to_owned(),
            uid_validity: "10".to_owned(),
            uid: 7,
            message_id: "<selected@example.test>".to_owned(),
        },
        status: ArchiveActionStatus::Restored,
        destination: Some(ArchiveLocation {
            folder: "Archive".to_owned(),
            uid_validity: "20".to_owned(),
            uid: 12,
        }),
        restored_location: Some(ArchiveLocation {
            folder: "INBOX".to_owned(),
            uid_validity: "10".to_owned(),
            uid: 8,
        }),
        attempts: 1,
        next_attempt_at: 0,
        created_at: 0,
        updated_at: 1,
        snapshot: None,
        restore_snapshot: None,
        reason: None,
        extra: Extra::new(),
    };
    let value = serialize_action(&action);
    assert_eq!(
        value["source"],
        json!({"folder": "INBOX", "uidValidity": "10", "uid": 7, "messageId": "<selected@example.test>"})
    );
    assert_eq!(
        value["destination"],
        json!({"folder": "Archive", "uidValidity": "20", "uid": 12})
    );
    assert_eq!(
        value["restoredLocation"],
        json!({"folder": "INBOX", "uidValidity": "10", "uid": 8})
    );
}

#[tokio::test]
async fn queue_status_cancel_and_restore_tools_follow_the_receipt() {
    let clock: SharedClock = TestClock::new(1_790_769_600_000);
    let store = TestStore::new(clock.clone()).await;
    let tools = email_tools(ToolDeps {
        compose: ComposeService::new(store.store.clone(), clock.clone(), None, None),
        archive: ArchiveService::new(store.store.clone(), clock),
        archive_transport: None,
        attachments: None,
        tracker: TaskTracker::new(),
    })
    .unwrap();
    let call = |name: &'static str, input: Value| {
        let tool = tools.iter().find(|t| t.meta.name == name).unwrap().clone();
        async move {
            let cx = ToolContext {
                call_id: "t".to_owned(),
                cancel: CancellationToken::new(),
            };
            tool.handler.call(input, cx).await.map(|out| match out {
                ToolOutput::Structured(map) => Value::Object(map),
                ToolOutput::Custom { structured, .. } => Value::Object(structured),
            })
        }
    };
    let queued = call(
        "email_archive_queue",
        json!({
            "idempotencyKey": " key ",
            "origin": {"folder": "INBOX", "uidValidity": "10", "uid": 7},
            "messageId": "<one@example.test>",
        }),
    )
    .await
    .unwrap();
    assert_eq!(queued["status"], json!("queued"));
    assert_eq!(queued["reason"], Value::Null);
    let id = queued["actionId"].as_str().unwrap().to_owned();
    // The trimmed key reaches the same receipt.
    let again = call(
        "email_archive_queue",
        json!({
            "idempotencyKey": "key",
            "origin": {"folder": "INBOX", "uidValidity": "10", "uid": 7},
            "messageId": "<one@example.test>",
        }),
    )
    .await
    .unwrap();
    assert_eq!(again["actionId"], json!(id));
    let status = call("email_archive_status", json!({"actionId": id}))
        .await
        .unwrap();
    assert_eq!(status["status"], json!("queued"));
    let restore = call("email_archive_restore", json!({"actionId": id}))
        .await
        .unwrap_err();
    assert_eq!(restore.message, "Email monitoring is not active");
    let cancelled = call("email_archive_cancel", json!({"actionId": id}))
        .await
        .unwrap();
    assert_eq!(cancelled["status"], json!("cancelled"));
    let twice = call("email_archive_cancel", json!({"actionId": id}))
        .await
        .unwrap();
    assert_eq!(twice["status"], json!("cancelled"));
    let rejected = call(
        "email_archive_queue",
        json!({
            "idempotencyKey": "other",
            "origin": {"folder": "Archive", "uidValidity": "10", "uid": 7},
            "messageId": "<one@example.test>",
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(rejected.phase, omni_mcp_kit::ToolPhase::Input);
    // Schema `\d` is ASCII-only: other Unicode digits are rejected as input.
    let non_ascii = call(
        "email_archive_queue",
        json!({
            "idempotencyKey": "unicode-digits",
            "origin": {"folder": "INBOX", "uidValidity": "\u{0661}\u{0662}", "uid": 7},
            "messageId": "<two@example.test>",
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(non_ascii.phase, omni_mcp_kit::ToolPhase::Input);
}
