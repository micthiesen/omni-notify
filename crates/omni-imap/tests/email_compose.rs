//! Port of `src/mcp/tools/email-compose.spec.ts` (compose MCP idempotency and
//! durable Sent recovery). The handlers run through their golden MCP
//! metadata; SMTP and the mailbox are scripted fakes.
//!
//! "keeps uncertain failures pending": TS interrupts the send Effect; here the
//! fake SMTP submission never completes and the caller times out, which
//! leaves the same durable `pending` reservation.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::FutureExt as _;
use futures::future::BoxFuture;
use omni_core::clock::{SharedClock, TestClock};
use omni_imap::archive_service::ArchiveService;
use omni_imap::compose::{ComposeMailbox, ComposeSender, ComposeService};
use omni_imap::mcp_tools::{ToolDeps, email_tools};
use omni_imap::ops::drafts::{EmailDraftInput, EmailDraftResult};
use omni_imap::ops::sent::{BeforeAppend, SentCopyInput, SentCopyResult};
use omni_imap::protocol::ImapError;
use omni_mcp_kit::{McpTool, ToolContext, ToolError, ToolOutput, ToolPhase};
use omni_testkit::TestStore;
use serde_json::{Value, json};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

const NOW: i64 = 1_790_769_600_000;

/// Scripted SMTP: each call pops the next outcome (or waits on `gate`).
#[derive(Default)]
struct FakeSmtp {
    calls: Mutex<usize>,
    outcomes: Mutex<Vec<bool>>,
    gate: Option<Arc<Notify>>,
    started: Arc<Notify>,
    hang: bool,
}

impl ComposeSender for FakeSmtp {
    fn send<'a>(&'a self, _recipients: &'a [String], _raw: &'a [u8]) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            *self.calls.lock().unwrap() += 1;
            self.started.notify_one();
            if self.hang {
                std::future::pending::<()>().await;
            }
            if let Some(gate) = &self.gate {
                gate.notified().await;
            }
            self.outcomes.lock().unwrap().pop().unwrap_or(true)
        })
    }
}

type CopyReply = Box<
    dyn Fn(Option<BeforeAppend>) -> BoxFuture<'static, Result<SentCopyResult, ImapError>>
        + Send
        + Sync,
>;
type DraftReply = Box<dyn Fn() -> Result<EmailDraftResult, ImapError> + Send + Sync>;

#[derive(Default)]
struct FakeMailbox {
    copy_calls: Mutex<Vec<(SentCopyInput, bool)>>,
    copy_replies: Mutex<Vec<CopyReply>>,
    draft_calls: Mutex<Vec<bool>>,
    draft_replies: Mutex<Vec<DraftReply>>,
}

impl ComposeMailbox for FakeMailbox {
    fn create_draft<'a>(
        &'a self,
        _input: &'a EmailDraftInput,
        allow_append: bool,
    ) -> BoxFuture<'a, Result<EmailDraftResult, ImapError>> {
        self.draft_calls.lock().unwrap().push(allow_append);
        let reply = self.draft_replies.lock().unwrap().remove(0);
        let result = reply();
        Box::pin(async move { result })
    }

    fn save_sent_copy<'a>(
        &'a self,
        input: &'a SentCopyInput,
        allow_append: bool,
        before_append: Option<BeforeAppend>,
    ) -> BoxFuture<'a, Result<SentCopyResult, ImapError>> {
        self.copy_calls
            .lock()
            .unwrap()
            .push((input.clone(), allow_append));
        let reply = self.copy_replies.lock().unwrap().remove(0);
        reply(before_append)
    }
}

fn copied(existed: bool) -> SentCopyResult {
    SentCopyResult {
        message_id: "<copy@test>".to_owned(),
        mailbox: "Sent".to_owned(),
        already_existed: existed,
    }
}

struct Env {
    _store: TestStore,
    tools: Vec<McpTool>,
}

async fn env(smtp: Arc<FakeSmtp>, mailbox: Option<Arc<FakeMailbox>>) -> Env {
    let clock: SharedClock = TestClock::new(NOW);
    let store = TestStore::new(clock.clone()).await;
    let compose = ComposeService::new(
        store.store.clone(),
        clock.clone(),
        Some(smtp as Arc<dyn ComposeSender>),
        mailbox.map(|m| m as Arc<dyn ComposeMailbox>),
    );
    let tools = email_tools(ToolDeps {
        compose,
        archive: ArchiveService::new(store.store.clone(), clock),
        archive_transport: None,
        attachments: None,
        tracker: TaskTracker::new(),
    })
    .unwrap();
    Env {
        _store: store,
        tools,
    }
}

impl Env {
    async fn call(&self, name: &str, input: Value) -> Result<Value, ToolError> {
        let tool = self.tools.iter().find(|t| t.meta.name == name).unwrap();
        let cx = ToolContext {
            call_id: "test".to_owned(),
            cancel: CancellationToken::new(),
        };
        match tool.handler.call(input, cx).await? {
            ToolOutput::Structured(map) => Ok(Value::Object(map)),
            ToolOutput::Custom { structured, .. } => Ok(Value::Object(structured)),
        }
    }
}

fn message(overrides: Value) -> Value {
    let mut base = json!({
        "idempotencyKey": "send-key",
        "to": "to@example.test",
        "subject": "Test",
        "text": "Body",
    });
    for (k, v) in overrides.as_object().unwrap() {
        base[k] = v.clone();
    }
    base
}

fn sends(smtp: &FakeSmtp) -> usize {
    *smtp.calls.lock().unwrap()
}

#[tokio::test]
async fn rejects_caller_selected_sender_fields_on_sends_replies_and_drafts() {
    let smtp = Arc::new(FakeSmtp::default());
    let e = env(smtp.clone(), Some(Arc::new(FakeMailbox::default()))).await;
    for tool in ["email_send", "email_draft_create"] {
        for field in ["from", "sender"] {
            let error = e
                .call(
                    tool,
                    message(json!({ field: "micthiesen@icloud.com", "inReplyTo": "<parent@example.test>" })),
                )
                .await
                .unwrap_err();
            assert_eq!(error.phase, ToolPhase::Input, "{tool} {field}");
        }
    }
    assert_eq!(sends(&smtp), 0);
}

#[tokio::test]
async fn returns_the_stored_success_on_retry_without_sending_twice() {
    let smtp = Arc::new(FakeSmtp::default());
    let e = env(smtp.clone(), None).await;
    let first = e.call("email_send", message(json!({}))).await.unwrap();
    let retry = e.call("email_send", message(json!({}))).await.unwrap();
    assert_eq!(first["sent"], json!(true));
    assert_eq!(first["alreadySent"], json!(false));
    let mut expected = first.clone();
    expected["alreadySent"] = json!(true);
    assert_eq!(retry, expected);
    assert_eq!(sends(&smtp), 1);
}

#[tokio::test]
async fn rejects_changed_content_for_a_previously_reserved_key() {
    let smtp = Arc::new(FakeSmtp::default());
    let e = env(smtp.clone(), None).await;
    e.call(
        "email_send",
        message(json!({"idempotencyKey": "conflict-key"})),
    )
    .await
    .unwrap();
    let error = e
        .call(
            "email_send",
            message(json!({"idempotencyKey": "conflict-key", "text": "Changed"})),
        )
        .await
        .unwrap_err();
    assert!(
        error.message.contains("different send content"),
        "{}",
        error.message
    );
    assert_eq!(sends(&smtp), 1);
}

#[tokio::test]
async fn keeps_uncertain_failures_pending_and_never_resends_automatically() {
    let smtp = Arc::new(FakeSmtp {
        hang: true,
        ..FakeSmtp::default()
    });
    let e = env(smtp.clone(), None).await;
    let first = tokio::time::timeout(
        Duration::from_millis(50),
        e.call(
            "email_send",
            message(json!({"idempotencyKey": "uncertain-key"})),
        ),
    )
    .await;
    assert!(first.is_err(), "the submission never completed");
    let error = e
        .call(
            "email_send",
            message(json!({"idempotencyKey": "uncertain-key"})),
        )
        .await
        .unwrap_err();
    assert!(
        error.message.contains("uncertain outcome"),
        "{}",
        error.message
    );
    assert_eq!(sends(&smtp), 1);
}

#[tokio::test]
async fn reconciles_a_pending_draft_with_allow_append_false() {
    let mailbox = Arc::new(FakeMailbox::default());
    mailbox.draft_replies.lock().unwrap().extend([
        Box::new(|| Err(ImapError::new("APPEND draft", "uncertain append"))) as DraftReply,
        Box::new(|| {
            Ok(EmailDraftResult {
                draft_id: "<draft@omni-notify>".to_owned(),
                already_existed: true,
            })
        }),
    ]);
    let e = env(Arc::new(FakeSmtp::default()), Some(mailbox.clone())).await;
    let input = message(json!({"idempotencyKey": "pending-draft"}));
    let error = e
        .call("email_draft_create", input.clone())
        .await
        .unwrap_err();
    assert_eq!(error.message, "uncertain append");
    let result = e.call("email_draft_create", input).await.unwrap();
    assert_eq!(
        result,
        json!({"draftId": "<draft@omni-notify>", "alreadyExisted": true})
    );
    assert_eq!(*mailbox.draft_calls.lock().unwrap(), vec![true, false]);
}

#[tokio::test]
async fn reserves_a_concurrent_key_once_so_only_one_send_reaches_smtp() {
    let gate = Arc::new(Notify::new());
    let smtp = Arc::new(FakeSmtp {
        gate: Some(gate.clone()),
        ..FakeSmtp::default()
    });
    let e = Arc::new(env(smtp.clone(), None).await);
    let first = {
        let e = e.clone();
        tokio::spawn(async move {
            e.call(
                "email_send",
                message(json!({"idempotencyKey": "concurrent-key"})),
            )
            .await
        })
    };
    smtp.started.notified().await;
    let error = e
        .call(
            "email_send",
            message(json!({"idempotencyKey": "concurrent-key"})),
        )
        .await
        .unwrap_err();
    assert!(
        error.message.contains("uncertain outcome"),
        "{}",
        error.message
    );
    gate.notify_one();
    let first = first.await.unwrap().unwrap();
    assert_eq!(
        (first["sent"].clone(), first["alreadySent"].clone()),
        (json!(true), json!(false))
    );
    assert_eq!(sends(&smtp), 1);
}

#[tokio::test]
async fn records_smtp_success_separately_then_only_reconciles_a_failed_copy_on_retry() {
    let mailbox = Arc::new(FakeMailbox::default());
    mailbox.copy_replies.lock().unwrap().extend([
        Box::new(|before: Option<BeforeAppend>| {
            async move {
                let claim = before.unwrap();
                assert!(claim().await.unwrap());
                Err(ImapError::new("APPEND Sent copy", "lost APPEND response"))
            }
            .boxed()
        }) as CopyReply,
        Box::new(|_| async { Ok(copied(true)) }.boxed()),
    ]);
    let smtp = Arc::new(FakeSmtp::default());
    let e = env(smtp.clone(), Some(mailbox.clone())).await;
    let input = message(json!({"idempotencyKey": "sent-recovery"}));
    let first = e.call("email_send", input.clone()).await.unwrap();
    assert_eq!(first["sentCopy"], json!("uncertain"));
    assert_eq!(first["alreadySent"], json!(false));
    let status = e
        .call(
            "email_send_status",
            json!({"idempotencyKey": "sent-recovery"}),
        )
        .await
        .unwrap();
    assert_eq!(
        (status["smtpAccepted"].clone(), status["sentCopy"].clone()),
        (json!(true), json!("uncertain"))
    );
    assert_eq!(mailbox.copy_calls.lock().unwrap().len(), 1);
    assert!(mailbox.copy_calls.lock().unwrap()[0].1);
    let repaired = e
        .call(
            "email_sent_copy_repair",
            json!({"idempotencyKey": "sent-recovery"}),
        )
        .await
        .unwrap();
    assert_eq!(repaired, json!({"sentCopy": "verified"}));
    {
        let calls = mailbox.copy_calls.lock().unwrap();
        assert!(!calls[1].1);
        assert_eq!(calls[1].0.content, calls[0].0.content);
    }
    let again = e.call("email_send", input).await.unwrap();
    assert_eq!(
        (again["alreadySent"].clone(), again["sentCopy"].clone()),
        (json!(true), json!("verified"))
    );
    assert_eq!(sends(&smtp), 1);
    assert_eq!(mailbox.copy_calls.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn never_appends_after_partial_smtp_acceptance_and_reports_missing_receipts_truthfully() {
    let mailbox = Arc::new(FakeMailbox::default());
    let smtp = Arc::new(FakeSmtp::default());
    smtp.outcomes.lock().unwrap().push(false);
    let e = env(smtp, Some(mailbox.clone())).await;
    let error = e
        .call(
            "email_send",
            message(json!({"idempotencyKey": "sent-partial"})),
        )
        .await
        .unwrap_err();
    assert!(
        error.message.contains("Some recipients may"),
        "{}",
        error.message
    );
    let error = e
        .call(
            "email_sent_copy_repair",
            json!({"idempotencyKey": "sent-partial"}),
        )
        .await
        .unwrap_err();
    assert!(error.message.contains("not confirmed"), "{}", error.message);
    assert!(mailbox.copy_calls.lock().unwrap().is_empty());
    let missing = e
        .call("email_send_status", json!({"idempotencyKey": "missing"}))
        .await
        .unwrap();
    assert_eq!(
        (
            missing["found"].clone(),
            missing["smtpAccepted"].clone(),
            missing["status"].clone()
        ),
        (json!(false), json!(false), Value::Null)
    );
}

#[tokio::test]
async fn keeps_pre_append_outages_repairable_without_retransmitting_smtp() {
    let mailbox = Arc::new(FakeMailbox::default());
    mailbox.copy_replies.lock().unwrap().extend([
        Box::new(|_| async { Err(ImapError::new("access connection", "disconnected")) }.boxed())
            as CopyReply,
        Box::new(|before: Option<BeforeAppend>| {
            async move {
                assert!(before.unwrap()().await.unwrap());
                Ok(copied(false))
            }
            .boxed()
        }),
    ]);
    let smtp = Arc::new(FakeSmtp::default());
    let e = env(smtp.clone(), Some(mailbox.clone())).await;
    let sent = e
        .call(
            "email_send",
            message(json!({"idempotencyKey": "disconnected-copy"})),
        )
        .await
        .unwrap();
    assert_eq!(sent["sentCopy"], json!("pending"));
    let repaired = e
        .call(
            "email_sent_copy_repair",
            json!({"idempotencyKey": "disconnected-copy"}),
        )
        .await
        .unwrap();
    assert_eq!(repaired, json!({"sentCopy": "verified"}));
    assert!(mailbox.copy_calls.lock().unwrap()[1].1);
    assert_eq!(sends(&smtp), 1);
}
