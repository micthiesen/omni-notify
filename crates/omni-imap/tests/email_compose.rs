//! Port of `src/mcp/tools/email-compose.spec.ts` (compose MCP idempotency and
//! durable Sent recovery). The handlers run through their golden MCP
//! metadata; SMTP and the mailbox are scripted fakes.
//!
//! "keeps uncertain failures pending": TS interrupts the send Effect; here the
//! fake SMTP submission never completes and the caller times out, which
//! leaves the same durable `pending` reservation.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::FutureExt as _;
use futures::future::BoxFuture;
use omni_core::clock::{SharedClock, TestClock};
use omni_core::email::DownloadedAttachment;
use omni_imap::archive_service::ArchiveService;
use omni_imap::attachments::AttachmentReader;
use omni_imap::compose::{ComposeKind, ComposeMailbox, ComposeSender, ComposeService, key_for};
use omni_imap::mcp_tools::{ToolDeps, email_tools};
use omni_imap::ops::drafts::{EmailDraftInput, EmailDraftResult};
use omni_imap::ops::sent::{BeforeAppend, SentCopyInput, SentCopyResult};
use omni_imap::protocol::ImapError;
use omni_mcp_kit::{McpTool, ToolContext, ToolError, ToolOutput, ToolPhase};
use omni_store::cbor::{self, JsValue};
use omni_store::{DocMeta, DocOps as _, DocWrite as _, Store, StoreError};
use omni_testkit::TestStore;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

const NOW: i64 = 1_790_769_600_000;

type SendHook = Box<dyn FnOnce() -> BoxFuture<'static, ()> + Send>;

/// Scripted SMTP: each call pops the next outcome (or waits on `gate`).
#[derive(Default)]
struct FakeSmtp {
    calls: Mutex<usize>,
    outcomes: Mutex<Vec<bool>>,
    gate: Option<Arc<Notify>>,
    started: Arc<Notify>,
    hang: bool,
    /// Every submission's recipients and wire MIME.
    submitted: Mutex<Vec<(Vec<String>, Vec<u8>)>>,
    /// Runs once during the next submission.
    hook: Mutex<Option<SendHook>>,
}

impl ComposeSender for FakeSmtp {
    fn send<'a>(&'a self, recipients: &'a [String], raw: &'a [u8]) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            *self.calls.lock().unwrap() += 1;
            self.submitted
                .lock()
                .unwrap()
                .push((recipients.to_vec(), raw.to_vec()));
            self.started.notify_one();
            let hook = self.hook.lock().unwrap().take();
            if let Some(hook) = hook {
                hook().await;
            }
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
type DraftReply =
    Box<dyn FnOnce() -> BoxFuture<'static, Result<EmailDraftResult, ImapError>> + Send>;

#[derive(Default)]
struct FakeMailbox {
    copy_calls: Mutex<Vec<(SentCopyInput, bool)>>,
    copy_replies: Mutex<Vec<CopyReply>>,
    draft_calls: Mutex<Vec<bool>>,
    draft_inputs: Mutex<Vec<EmailDraftInput>>,
    draft_replies: Mutex<Vec<DraftReply>>,
}

impl ComposeMailbox for FakeMailbox {
    fn create_draft<'a>(
        &'a self,
        input: &'a EmailDraftInput,
        allow_append: bool,
    ) -> BoxFuture<'a, Result<EmailDraftResult, ImapError>> {
        self.draft_calls.lock().unwrap().push(allow_append);
        self.draft_inputs.lock().unwrap().push(input.clone());
        let reply = self.draft_replies.lock().unwrap().remove(0);
        reply()
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
        let mut replies = self.copy_replies.lock().unwrap();
        if replies.is_empty() {
            // Unscripted: a pre-APPEND outage leaves the copy pending.
            return async { Err(ImapError::new("access connection", "not scripted")) }.boxed();
        }
        let reply = replies.remove(0);
        reply(before_append)
    }
}

fn draft_ok(draft_id: &str, already_existed: bool) -> DraftReply {
    let result = EmailDraftResult {
        draft_id: draft_id.to_owned(),
        already_existed,
    };
    Box::new(move || async move { Ok(result) }.boxed())
}

fn draft_err(detail: &str) -> DraftReply {
    let error = ImapError::new("APPEND draft", detail);
    Box::new(move || async move { Err(error) }.boxed())
}

fn copied(existed: bool) -> SentCopyResult {
    SentCopyResult {
        message_id: "<copy@test>".to_owned(),
        mailbox: "Sent".to_owned(),
        already_existed: existed,
    }
}

/// One scripted attachment source.
#[derive(Clone)]
enum Source {
    Missing,
    Fails(ImapError),
    Found(DownloadedAttachment),
}

/// Scripted stable attachment reads keyed by attachmentId.
#[derive(Default)]
struct FakeReader {
    sources: Mutex<HashMap<String, Source>>,
    calls: Mutex<Vec<(String, String, usize)>>,
}

impl FakeReader {
    fn with(sources: impl IntoIterator<Item = (String, Source)>) -> Arc<Self> {
        Arc::new(Self {
            sources: Mutex::new(sources.into_iter().collect()),
            calls: Mutex::new(Vec::new()),
        })
    }

    fn set(&self, attachment_id: &str, source: Source) {
        self.sources
            .lock()
            .unwrap()
            .insert(attachment_id.to_owned(), source);
    }

    fn calls(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}

impl AttachmentReader for FakeReader {
    fn fetch_attachment<'a>(
        &'a self,
        message_id: &'a str,
        attachment_id: &'a str,
        max_bytes: usize,
    ) -> BoxFuture<'a, Result<Option<DownloadedAttachment>, ImapError>> {
        self.calls.lock().unwrap().push((
            message_id.to_owned(),
            attachment_id.to_owned(),
            max_bytes,
        ));
        let source = self
            .sources
            .lock()
            .unwrap()
            .get(attachment_id)
            .cloned()
            .unwrap_or(Source::Missing);
        Box::pin(async move {
            match source {
                Source::Missing => Ok(None),
                Source::Fails(error) => Err(error),
                Source::Found(found) => Ok(Some(found)),
            }
        })
    }
}

struct Env {
    store: TestStore,
    tools: Vec<McpTool>,
}

async fn env(smtp: Arc<FakeSmtp>, mailbox: Option<Arc<FakeMailbox>>) -> Env {
    env_with(smtp, mailbox, None).await
}

async fn env_with(
    smtp: Arc<FakeSmtp>,
    mailbox: Option<Arc<FakeMailbox>>,
    reader: Option<Arc<FakeReader>>,
) -> Env {
    let clock: SharedClock = TestClock::new(NOW);
    let store = TestStore::new(clock.clone()).await;
    let compose = ComposeService::new(
        store.store.clone(),
        clock.clone(),
        Some(smtp as Arc<dyn ComposeSender>),
        mailbox.map(|m| m as Arc<dyn ComposeMailbox>),
    )
    .with_attachment_reader(reader.map(|r| r as Arc<dyn AttachmentReader>));
    let tools = email_tools(ToolDeps {
        compose,
        archive: ArchiveService::new(store.store.clone(), clock),
        archive_transport: None,
        attachments: None,
        tracker: TaskTracker::new(),
    })
    .unwrap();
    Env { store, tools }
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

    /// The stored receipt as JSON (`readReceipt`).
    async fn receipt(&self, kind: ComposeKind, key: &str) -> Option<Value> {
        read_receipt(&self.store.store, kind, key).await
    }

    /// Replaces a send receipt (`writeReceipt`).
    async fn write_receipt(&self, key: &str, data: Value) {
        write_receipt(&self.store.store, key, data).await;
    }
}

async fn read_receipt(store: &Store, kind: ComposeKind, key: &str) -> Option<Value> {
    let pk = key_for(kind, key);
    let value: Option<JsValue> = store
        .read(move |docs| docs.get_raw_row(&pk)?.map(|row| row.decode()).transpose())
        .await
        .unwrap();
    value.map(|v| cbor::from_value::<Value>(v).unwrap())
}

async fn write_receipt(store: &Store, key: &str, data: Value) {
    let pk = key_for(ComposeKind::Send, key);
    let value = cbor::to_value(&data).unwrap();
    store
        .write(move |tx| -> Result<(), StoreError> {
            tx.upsert_doc(
                &pk,
                &value,
                DocMeta {
                    entity: Some("email-compose-send".to_owned()),
                    ..DocMeta::default()
                },
            )
        })
        .await
        .unwrap();
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

fn sha256(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
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
        draft_err("uncertain append"),
        draft_ok("<draft@omni-notify>", true),
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
        json!({"draftId": "<draft@omni-notify>", "alreadyExisted": true, "attachments": []})
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

#[tokio::test]
async fn replays_a_legacy_receipt_under_the_fingerprint_computed_before_attachments() {
    let smtp = Arc::new(FakeSmtp::default());
    let e = env(smtp.clone(), None).await;
    // Parsed field order and sender exactly as hashed before attachments existed.
    let legacy = sha256(
        br#"{"idempotencyKey":"legacy-key","to":["to@example.test"],"cc":["cc@example.test"],"subject":"Test","text":"Body","inReplyTo":"<parent@example.test>","references":["<root@example.test>"],"from":"michael@thiesen.dev"}"#,
    );
    e.write_receipt(
        "legacy-key",
        json!({
            "fingerprint": legacy,
            "status": "succeeded",
            "result": {"sent": true, "messageId": format!("<{legacy}@omni-notify>")},
            "prepared": {
                "from": "michael@thiesen.dev",
                "date": "2026-10-01T00:00:00.000Z",
                "wire": "d2lyZQ==",
                "content": "Y29weQ==",
            },
            "sentCopy": "verified",
            "updatedAt": 1,
        }),
    )
    .await;
    let input = message(json!({
        "idempotencyKey": "legacy-key",
        "cc": ["cc@example.test"],
        "inReplyTo": "<parent@example.test>",
        "references": ["<root@example.test>"],
    }));
    let mut empty = input.clone();
    empty["attachments"] = json!([]);
    for replay in [input, empty] {
        assert_eq!(
            e.call("email_send", replay).await.unwrap(),
            json!({
                "sent": true,
                "messageId": format!("<{legacy}@omni-notify>"),
                "alreadySent": true,
                "sentCopy": "verified",
                "attachments": [],
            })
        );
    }
    assert_eq!(sends(&smtp), 0);
}

#[tokio::test]
async fn keeps_do_not_repeat_guidance_visible_when_an_accepted_send_cannot_be_recorded() {
    let key = "unrecordable-key";
    let smtp = Arc::new(FakeSmtp::default());
    let e = env(smtp.clone(), None).await;
    let store = e.store.store.clone();
    *smtp.hook.lock().unwrap() = Some(Box::new(move || {
        async move {
            write_receipt(
                &store,
                key,
                json!({"fingerprint": "other", "status": "pending", "updatedAt": 1}),
            )
            .await;
        }
        .boxed()
    }));
    let error = e
        .call("email_send", message(json!({"idempotencyKey": key})))
        .await
        .unwrap_err();
    assert!(
        error
            .message
            .starts_with("Could not persist email send outcome; do not repeat with a new key: "),
        "{}",
        error.message
    );
}

#[tokio::test]
async fn completes_a_draft_reconciled_after_a_concurrent_caller_recorded_it() {
    let draft_id = "<draft-race@omni-notify>";
    let (first_tx, first_rx) = tokio::sync::oneshot::channel::<()>();
    let (second_tx, second_rx) = tokio::sync::oneshot::channel::<()>();
    let mailbox = Arc::new(FakeMailbox::default());
    mailbox.draft_replies.lock().unwrap().extend([
        Box::new(move || {
            async move {
                first_rx.await.unwrap();
                Ok(EmailDraftResult {
                    draft_id: draft_id.to_owned(),
                    already_existed: false,
                })
            }
            .boxed()
        }) as DraftReply,
        Box::new(move || {
            async move {
                second_rx.await.unwrap();
                Ok(EmailDraftResult {
                    draft_id: draft_id.to_owned(),
                    already_existed: true,
                })
            }
            .boxed()
        }),
    ]);
    let e = Arc::new(env(Arc::new(FakeSmtp::default()), Some(mailbox.clone())).await);
    let input = message(json!({"idempotencyKey": "draft-race"}));
    let calls = |n: usize| {
        let mailbox = mailbox.clone();
        async move {
            while mailbox.draft_calls.lock().unwrap().len() < n {
                tokio::task::yield_now().await;
            }
        }
    };
    let winner = {
        let (e, input) = (e.clone(), input.clone());
        tokio::spawn(async move { e.call("email_draft_create", input).await })
    };
    calls(1).await;
    let reconciler = {
        let (e, input) = (e.clone(), input.clone());
        tokio::spawn(async move { e.call("email_draft_create", input).await })
    };
    calls(2).await;
    assert_eq!(*mailbox.draft_calls.lock().unwrap(), vec![true, false]);
    first_tx.send(()).unwrap();
    let won = winner.await.unwrap().unwrap();
    assert_eq!(
        (won["draftId"].clone(), won["alreadyExisted"].clone()),
        (json!(draft_id), json!(false))
    );
    second_tx.send(()).unwrap();
    let reconciled = reconciler.await.unwrap().unwrap();
    assert_eq!(
        (
            reconciled["draftId"].clone(),
            reconciled["alreadyExisted"].clone()
        ),
        (json!(draft_id), json!(true))
    );
}

// email attachments by stable reference

const MIB: usize = 1024 * 1024;
const SOURCE: &str = "<source@example.test>";

fn id_for(n: u32) -> String {
    format!("imap-attachment:{n:064x}")
}

/// Without reviewed bytes, a well-formed pin that no fixture matches.
fn reference(n: u32, reviewed: Option<&[u8]>) -> Value {
    json!({
        "messageId": SOURCE,
        "attachmentId": id_for(n),
        "sha256": reviewed.map_or_else(|| "0".repeat(64), sha256),
    })
}

fn pdf(label: &str, size: Option<usize>) -> Vec<u8> {
    let mut bytes = format!("%PDF-1.7\n{label} synthetic fixture\n%%EOF").into_bytes();
    if let Some(size) = size {
        bytes.resize(size, 0);
    }
    bytes
}

fn found(name: &str, mime_type: &str, data: &[u8]) -> Source {
    Source::Found(DownloadedAttachment {
        name: name.to_owned(),
        mime_type: mime_type.to_owned(),
        data: data.to_vec(),
    })
}

fn metadata_for(n: u32, filename: &str, data: &[u8]) -> Value {
    json!({
        "messageId": SOURCE,
        "attachmentId": id_for(n),
        "filename": filename,
        "mimeType": "application/pdf",
        "size": data.len(),
        "sha256": sha256(data),
    })
}

struct AttachFixture {
    e: Env,
    reader: Arc<FakeReader>,
    smtp: Arc<FakeSmtp>,
    mailbox: Arc<FakeMailbox>,
}

async fn attach_fixture(sources: Vec<(String, Source)>) -> AttachFixture {
    let reader = FakeReader::with(sources);
    let smtp = Arc::new(FakeSmtp::default());
    let mailbox = Arc::new(FakeMailbox::default());
    let e = env_with(smtp.clone(), Some(mailbox.clone()), Some(reader.clone())).await;
    AttachFixture {
        e,
        reader,
        smtp,
        mailbox,
    }
}

fn parsed_wire(smtp: &FakeSmtp, index: usize) -> omni_imap::mime::ParsedMail {
    let raw = smtp.submitted.lock().unwrap()[index].1.clone();
    omni_imap::mime::parse_message(&raw, NOW).unwrap()
}

fn text_of(parsed: &omni_imap::mime::ParsedMail) -> String {
    parsed
        .text
        .clone()
        .unwrap_or_default()
        .replace("\r\n", "\n")
        .trim_end()
        .to_owned()
}

#[tokio::test]
async fn sends_several_re_read_pdfs_in_the_persisted_wire_mime_with_safe_names_and_threading() {
    let lab = pdf("lab", None);
    let scan = pdf("scan", None);
    let f = attach_fixture(vec![
        (
            id_for(1),
            found("../Lab \"results\".pdf", "application/pdf", &lab),
        ),
        (id_for(2), found("scan", "application/pdf", &scan)),
    ])
    .await;
    let result =
        f.e.call(
            "email_send",
            message(json!({
                "idempotencyKey": "attach-send",
                "subject": "Re: Results",
                "bcc": ["hidden@example.test"],
                "inReplyTo": "<parent@example.test>",
                "references": ["<root@example.test>"],
                "attachments": [reference(1, Some(&lab)), reference(2, Some(&scan))],
            })),
        )
        .await
        .unwrap();
    assert_eq!(
        *f.reader.calls.lock().unwrap(),
        vec![
            (SOURCE.to_owned(), id_for(1), 5 * MIB),
            (SOURCE.to_owned(), id_for(2), 5 * MIB),
        ]
    );
    let expected = json!([
        metadata_for(1, "Lab \"results\".pdf", &lab),
        metadata_for(2, "scan.pdf", &scan),
    ]);
    assert_eq!(result["sent"], json!(true));
    assert_eq!(result["alreadySent"], json!(false));
    assert_eq!(result["attachments"], expected);

    let recipients = f.smtp.submitted.lock().unwrap()[0].0.clone();
    assert!(recipients.contains(&"hidden@example.test".to_owned()));
    let wire = parsed_wire(&f.smtp, 0);
    assert_eq!(wire.message_id.as_deref(), result["messageId"].as_str());
    assert!(wire.bcc.is_none());
    assert_eq!(wire.subject.as_deref(), Some("Re: Results"));
    assert_eq!(text_of(&wire), "Body");
    assert_eq!(wire.in_reply_to.as_deref(), Some("<parent@example.test>"));
    assert_eq!(
        wire.references,
        Some(vec![
            "<root@example.test>".to_owned(),
            "<parent@example.test>".to_owned()
        ])
    );
    let parts: Vec<Value> = wire
        .attachments
        .iter()
        .map(|part| {
            json!([
                part.filename,
                part.content_type,
                part.content_disposition,
                sha256(&part.content)
            ])
        })
        .collect();
    let wanted: Vec<Value> = expected
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            json!([
                item["filename"],
                "application/pdf",
                "attachment",
                item["sha256"]
            ])
        })
        .collect();
    assert_eq!(parts, wanted);
    let status =
        f.e.call(
            "email_send_status",
            json!({"idempotencyKey": "attach-send"}),
        )
        .await
        .unwrap();
    assert_eq!(status["smtpAccepted"], json!(true));
    assert_eq!(status["attachments"], expected);
}

#[tokio::test]
async fn keeps_attachment_free_sends_compatible_and_treats_an_empty_list_as_none() {
    let f = attach_fixture(Vec::new()).await;
    let first =
        f.e.call(
            "email_send",
            message(json!({"idempotencyKey": "attach-compat"})),
        )
        .await
        .unwrap();
    let retry =
        f.e.call(
            "email_send",
            message(json!({"idempotencyKey": "attach-compat", "attachments": []})),
        )
        .await
        .unwrap();
    assert_eq!(first["alreadySent"], json!(false));
    assert_eq!(first["attachments"], json!([]));
    let mut expected = first.clone();
    expected["alreadySent"] = json!(true);
    assert_eq!(retry, expected);
    assert_eq!(f.reader.calls(), 0);
    assert_eq!(sends(&f.smtp), 1);
}

#[tokio::test]
async fn returns_the_stored_receipt_on_retry_without_re_reading_sources_or_resending() {
    let invoice = pdf("a", None);
    let other = pdf("b", None);
    let f = attach_fixture(vec![
        (id_for(3), found("invoice.pdf", "application/pdf", &invoice)),
        (id_for(4), found("other.pdf", "application/pdf", &other)),
    ])
    .await;
    let input = message(json!({
        "idempotencyKey": "attach-retry",
        "attachments": [reference(3, Some(&invoice))],
    }));
    let first = f.e.call("email_send", input.clone()).await.unwrap();
    f.reader.set(&id_for(3), Source::Missing);
    let mut expected = first.clone();
    expected["alreadySent"] = json!(true);
    assert_eq!(
        f.e.call("email_send", input.clone()).await.unwrap(),
        expected
    );
    let mut changed = input;
    changed["attachments"] = json!([reference(4, Some(&other))]);
    let error = f.e.call("email_send", changed).await.unwrap_err();
    assert!(
        error.message.contains("different send content"),
        "{}",
        error.message
    );
    assert_eq!(f.reader.calls(), 1);
    assert_eq!(sends(&f.smtp), 1);
}

#[tokio::test]
async fn never_re_reads_or_resends_after_an_uncertain_smtp_outcome() {
    let data = pdf("a", None);
    let reader = FakeReader::with([(id_for(5), found("a.pdf", "application/pdf", &data))]);
    let smtp = Arc::new(FakeSmtp {
        hang: true,
        ..FakeSmtp::default()
    });
    let e = env_with(smtp.clone(), None, Some(reader.clone())).await;
    let input = message(json!({
        "idempotencyKey": "attach-uncertain",
        "attachments": [reference(5, Some(&data))],
    }));
    let first = tokio::time::timeout(
        Duration::from_millis(50),
        e.call("email_send", input.clone()),
    )
    .await;
    assert!(first.is_err(), "the submission never completed");
    let error = e.call("email_send", input).await.unwrap_err();
    assert!(
        error.message.contains("uncertain outcome"),
        "{}",
        error.message
    );
    assert_eq!(reader.calls(), 1);
    assert_eq!(sends(&smtp), 1);
}

#[tokio::test]
async fn rejects_malformed_references_and_caller_supplied_bytes_before_imap_or_smtp() {
    let f = attach_fixture(Vec::new()).await;
    let with = |extra: Value| {
        let mut item = reference(1, None);
        for (k, v) in extra.as_object().unwrap() {
            item[k] = v.clone();
        }
        item
    };
    for attachments in [
        json!([{"messageId": SOURCE, "attachmentId": "../private.pdf"}]),
        json!([{"messageId": "source@example.test", "attachmentId": id_for(1)}]),
        json!([{"messageId": "<a@b>\r\nBcc: x@example.test", "attachmentId": id_for(1)}]),
        json!([reference(1, None), reference(1, None)]),
        Value::Array((1..=6).map(|n| reference(n, None)).collect()),
        json!([with(json!({"content": "JVBERi0="}))]),
        json!([with(json!({"path": "/etc/passwd"}))]),
        json!([with(json!({"filename": "renamed.pdf"}))]),
        json!([{"messageId": SOURCE, "attachmentId": id_for(1)}]),
        json!([with(json!({"sha256": "A".repeat(64)}))]),
        json!([with(json!({"sha256": "abc"}))]),
        reference(1, None),
    ] {
        for tool in ["email_send", "email_draft_create"] {
            let error =
                f.e.call(
                    tool,
                    message(
                        json!({"idempotencyKey": "attach-malformed", "attachments": attachments}),
                    ),
                )
                .await
                .unwrap_err();
            assert_eq!(error.phase, ToolPhase::Input, "{tool} {attachments}");
        }
    }
    // A messageId that is valid only after trimming is accepted like zod's trim.
    let mut padded = reference(1, None);
    padded["messageId"] = json!(format!("  {SOURCE} "));
    let error =
        f.e.call(
            "email_send",
            message(json!({"idempotencyKey": "attach-padded", "attachments": [padded]})),
        )
        .await
        .unwrap_err();
    assert_eq!(error.phase, ToolPhase::Execute);
    assert_eq!(
        *f.reader.calls.lock().unwrap(),
        vec![(SOURCE.to_owned(), id_for(1), 5 * MIB)]
    );
    assert_eq!(sends(&f.smtp), 0);
}

#[tokio::test]
async fn fails_before_reserving_for_missing_stale_non_pdf_and_oversized_sources() {
    let name = "Private Diagnosis.pdf";
    let valid = |data: Vec<u8>| found(name, "application/pdf", &data);
    let socket = ImapError::wrap(
        "find attachment message",
        ImapError::new("UID SEARCH", "socket closed"),
    );
    let cases: Vec<(Vec<Source>, &str, bool)> = vec![
        (
            vec![Source::Missing],
            "was not found in Inbox, Archive or Sent",
            false,
        ),
        (vec![Source::Fails(socket)], "could not be read", false),
        (
            vec![found(name, "text/html", &pdf("html", None))],
            "not a PDF",
            false,
        ),
        (vec![valid(b"plain text".to_vec())], "not a PDF", false),
        (
            vec![valid(pdf("big", Some(5 * MIB + 1)))],
            "5 MiB attachment limit",
            false,
        ),
        (
            (1..=3)
                .map(|n| valid(pdf(&n.to_string(), Some(4 * MIB))))
                .collect(),
            "10 MiB total limit",
            true,
        ),
        (
            vec![valid(pdf("changed", None))],
            "no longer matches the reviewed sha256",
            false,
        ),
    ];
    let smtp = Arc::new(FakeSmtp::default());
    for (index, (sources, error, pinned)) in cases.into_iter().enumerate() {
        let key = format!("attach-invalid-{index}");
        let ids: Vec<u32> = (0..sources.len()).map(|n| n as u32 + 10).collect();
        let references: Vec<Value> = sources
            .iter()
            .zip(&ids)
            .map(|(source, n)| match source {
                // Pin the reviewed bytes only where a later check must be reached.
                Source::Found(found) if pinned => reference(*n, Some(&found.data)),
                _ => reference(*n, None),
            })
            .collect();
        let reader = FakeReader::with(ids.iter().map(|n| id_for(*n)).zip(sources));
        let e = env_with(smtp.clone(), None, Some(reader)).await;
        let reason = e
            .call(
                "email_send",
                message(json!({"idempotencyKey": key, "attachments": references})),
            )
            .await
            .unwrap_err()
            .message;
        assert!(reason.contains(error), "{reason}");
        assert!(reason.contains("Nothing was sent or saved"), "{reason}");
        assert!(!reason.contains("Private Diagnosis"), "{reason}");
        let status = e
            .call("email_send_status", json!({"idempotencyKey": key}))
            .await
            .unwrap();
        assert_eq!(status["found"], json!(false));
    }
    assert_eq!(sends(&smtp), 0);

    let fixed = pdf("fixed", None);
    let reader = FakeReader::with([(id_for(10), valid(fixed.clone()))]);
    let e = env_with(smtp.clone(), None, Some(reader)).await;
    let recovered = e
        .call(
            "email_send",
            message(json!({
                "idempotencyKey": "attach-invalid-0",
                "attachments": [reference(10, Some(&fixed))],
            })),
        )
        .await
        .unwrap();
    assert_eq!(
        (recovered["sent"].clone(), recovered["alreadySent"].clone()),
        (json!(true), json!(false))
    );
}

#[tokio::test]
async fn refuses_attachments_when_stable_retrieval_is_unavailable() {
    let smtp = Arc::new(FakeSmtp::default());
    let e = env(smtp.clone(), None).await;
    let error = e
        .call(
            "email_send",
            message(json!({
                "idempotencyKey": "attach-unavailable",
                "attachments": [reference(1, None)],
            })),
        )
        .await
        .unwrap_err();
    assert!(
        error.message.contains("retrieval is unavailable"),
        "{}",
        error.message
    );
    assert_eq!(sends(&smtp), 0);
}

#[tokio::test]
async fn repairs_an_uncertain_sent_copy_from_persisted_attachment_mime_without_resending() {
    let data = pdf("sent-copy", None);
    let f = attach_fixture(vec![(
        id_for(6),
        found("statement.pdf", "application/pdf", &data),
    )])
    .await;
    f.mailbox.copy_replies.lock().unwrap().extend([
        Box::new(|before: Option<BeforeAppend>| {
            async move {
                assert!(before.unwrap()().await.unwrap());
                Err(ImapError::new("APPEND Sent copy", "lost APPEND response"))
            }
            .boxed()
        }) as CopyReply,
        Box::new(|_| async { Ok(copied(true)) }.boxed()),
    ]);
    let input = message(json!({
        "idempotencyKey": "attach-sent-copy",
        "bcc": ["hidden@example.test"],
        "attachments": [reference(6, Some(&data))],
    }));
    let sent = f.e.call("email_send", input).await.unwrap();
    assert_eq!(
        (sent["sent"].clone(), sent["sentCopy"].clone()),
        (json!(true), json!("uncertain"))
    );
    let pending =
        f.e.receipt(ComposeKind::Send, "attach-sent-copy")
            .await
            .unwrap();
    assert!(pending["prepared"].get("wire").is_none());
    assert!(pending["prepared"].get("content").is_some());
    assert_eq!(
        f.e.call(
            "email_sent_copy_repair",
            json!({"idempotencyKey": "attach-sent-copy"})
        )
        .await
        .unwrap(),
        json!({"sentCopy": "verified"})
    );
    let verified =
        f.e.receipt(ComposeKind::Send, "attach-sent-copy")
            .await
            .unwrap();
    let keys: Vec<&String> = verified["prepared"].as_object().unwrap().keys().collect();
    assert_eq!(keys, vec!["from", "date"]);
    let status =
        f.e.call(
            "email_send_status",
            json!({"idempotencyKey": "attach-sent-copy"}),
        )
        .await
        .unwrap();
    assert_eq!(status["sentCopy"], json!("verified"));
    assert!(status["messageDate"].is_string());
    assert_eq!(
        status["attachments"],
        json!([metadata_for(6, "statement.pdf", &data)])
    );
    let calls = f.mailbox.copy_calls.lock().unwrap().clone();
    assert!(!calls[1].1);
    assert_eq!(calls[1].0.content, calls[0].0.content);
    let saved = omni_imap::mime::parse_message(&calls[1].0.content, NOW).unwrap();
    assert_eq!(
        omni_imap::mime::ParsedMail::flat_addresses(saved.bcc.as_ref()),
        vec!["hidden@example.test"]
    );
    let parts: Vec<(Option<String>, String)> = saved
        .attachments
        .iter()
        .map(|part| (part.filename.clone(), sha256(&part.content)))
        .collect();
    assert_eq!(
        parts,
        vec![(Some("statement.pdf".to_owned()), sha256(&data))]
    );
    assert_eq!(f.reader.calls(), 1);
    assert_eq!(sends(&f.smtp), 1);
}

#[tokio::test]
async fn drafts_re_read_attachments_once_and_reconcile_a_pending_draft_from_its_receipt() {
    let data = pdf("draft", None);
    let f = attach_fixture(vec![(id_for(7), found("form", "application/pdf", &data))]).await;
    f.mailbox.draft_replies.lock().unwrap().extend([
        draft_err("uncertain append"),
        draft_ok("<draft-attach@omni-notify>", true),
    ]);
    let input = message(json!({
        "idempotencyKey": "attach-draft",
        "attachments": [reference(7, Some(&data))],
    }));
    let error =
        f.e.call("email_draft_create", input.clone())
            .await
            .unwrap_err();
    assert!(
        error.message.contains("uncertain append"),
        "{}",
        error.message
    );
    assert_eq!(
        f.mailbox.draft_inputs.lock().unwrap()[0].attachments,
        vec![omni_mailer::OutgoingEmailAttachment {
            filename: "form.pdf".to_owned(),
            content_type: "application/pdf".to_owned(),
            content: data.clone(),
        }]
    );
    let reconciled = f.e.call("email_draft_create", input.clone()).await.unwrap();
    assert_eq!(
        reconciled,
        json!({
            "draftId": "<draft-attach@omni-notify>",
            "alreadyExisted": true,
            "attachments": [metadata_for(7, "form.pdf", &data)],
        })
    );
    assert_eq!(*f.mailbox.draft_calls.lock().unwrap(), vec![true, false]);
    assert_eq!(
        f.e.call("email_draft_create", input).await.unwrap(),
        reconciled
    );
    assert_eq!(f.mailbox.draft_calls.lock().unwrap().len(), 2);
    assert_eq!(f.reader.calls(), 1);
}

#[tokio::test]
async fn reserves_no_draft_for_a_missing_source_then_returns_attached_metadata() {
    let data = pdf("draft-ok", None);
    let f = attach_fixture(vec![(id_for(9), Source::Missing)]).await;
    f.mailbox
        .draft_replies
        .lock()
        .unwrap()
        .push(draft_ok("<draft-ok@omni-notify>", false));
    let input = message(json!({
        "idempotencyKey": "attach-draft-ok",
        "attachments": [reference(9, Some(&data))],
    }));
    let error =
        f.e.call("email_draft_create", input.clone())
            .await
            .unwrap_err();
    assert!(error.message.contains("was not found"), "{}", error.message);
    assert!(f.mailbox.draft_calls.lock().unwrap().is_empty());
    assert!(
        f.e.receipt(ComposeKind::Draft, "attach-draft-ok")
            .await
            .is_none()
    );
    f.reader
        .set(&id_for(9), found("lease.pdf", "application/pdf", &data));
    assert_eq!(
        f.e.call("email_draft_create", input).await.unwrap(),
        json!({
            "draftId": "<draft-ok@omni-notify>",
            "alreadyExisted": false,
            "attachments": [metadata_for(9, "lease.pdf", &data)],
        })
    );
}

#[tokio::test]
async fn breaks_up_encoded_word_markers_so_recipients_cannot_decode_header_text() {
    let data = pdf("encoded", None);
    let f = attach_fixture(vec![(
        id_for(11),
        found(
            "=?utf-8?Q?evil=0D=0ABcc:x@y?=.pdf",
            "application/pdf",
            &data,
        ),
    )])
    .await;
    f.e.call(
        "email_send",
        message(json!({
            "idempotencyKey": "attach-encoded",
            "attachments": [reference(11, Some(&data))],
        })),
    )
    .await
    .unwrap();
    let wire = parsed_wire(&f.smtp, 0);
    let names: Vec<Option<String>> = wire
        .attachments
        .iter()
        .map(|part| part.filename.clone())
        .collect();
    assert_eq!(
        names,
        vec![Some("=_utf-8?Q?evil=0D=0ABcc:x@y?=.pdf".to_owned())]
    );
}
