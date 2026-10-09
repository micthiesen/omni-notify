//! Port of `src/email/imap/actions.spec.ts` (`createDraftEffect`).
//!
//! "releases the Drafts lock after failures": there is no lock object in
//! Rust (selection is exclusive `&mut` access), so the case asserts the
//! failure and that no APPEND happened.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_imap::fake::{FakeCall, FakeMessage, FakeOp, FakeServer};
use omni_imap::mime::parse_message;
use omni_imap::ops::drafts::{EmailDraftInput, create_draft, deterministic_draft_message_id};
use omni_imap::protocol::ImapError;

const NOW: i64 = 1_790_769_600_000;

fn input() -> EmailDraftInput {
    EmailDraftInput {
        idempotency_key: "draft-key".to_owned(),
        to: vec!["to@example.test".to_owned()],
        cc: Some(vec!["cc@example.test".to_owned()]),
        bcc: Some(vec!["hidden@example.test".to_owned()]),
        subject: "Reply draft".to_owned(),
        text: "A draft body".to_owned(),
        in_reply_to: Some("<parent@example.test>".to_owned()),
        references: Some(vec![
            "<root@example.test>".to_owned(),
            "<parent@example.test>".to_owned(),
        ]),
    }
}

fn server() -> FakeServer {
    let server = FakeServer::default();
    server
        .folder("INBOX", 1, None)
        .folder("Drafts.localized", 5, Some("\\Drafts"));
    server
}

fn appends(server: &FakeServer) -> usize {
    server.count(|c| matches!(c, FakeCall::Append { .. }))
}

#[tokio::test]
async fn discovers_the_special_use_drafts_mailbox_and_serializes_bcc_and_reply_headers() {
    let server = server();
    let mut client = server.client();
    let result = create_draft(&mut client, &input(), true, NOW)
        .await
        .unwrap();
    assert_eq!(result.draft_id, deterministic_draft_message_id("draft-key"));
    assert!(!result.already_existed);
    assert!(server.calls().contains(&FakeCall::Select {
        path: "Drafts.localized".to_owned(),
        read_only: false
    }));
    assert!(server.calls().contains(&FakeCall::Append {
        path: "Drafts.localized".to_owned(),
        flags: vec!["\\Draft".to_owned()],
        internal_date_ms: None,
    }));
    let stored = server.message("Drafts.localized", 1).unwrap();
    let parsed = parse_message(&stored.source, NOW).unwrap();
    let from = parsed.from.unwrap();
    assert_eq!(from.len(), 1);
    assert_eq!(from[0].address.as_deref(), Some("michael@thiesen.dev"));
    assert_eq!(from[0].name, "");
    let flat = |v: Option<Vec<Vec<omni_imap::mime::Address>>>| -> Vec<String> {
        v.unwrap_or_default()
            .into_iter()
            .flatten()
            .filter_map(|a| a.address)
            .collect()
    };
    assert_eq!(flat(parsed.to), vec!["to@example.test"]);
    assert_eq!(flat(parsed.cc), vec!["cc@example.test"]);
    assert_eq!(flat(parsed.bcc), vec!["hidden@example.test"]);
    assert_eq!(parsed.in_reply_to.as_deref(), Some("<parent@example.test>"));
    assert_eq!(
        parsed.references,
        Some(vec![
            "<root@example.test>".to_owned(),
            "<parent@example.test>".to_owned()
        ])
    );
}

#[tokio::test]
async fn releases_the_drafts_lock_after_failures() {
    let server = server();
    server.fail_next(
        FakeOp::Search,
        None,
        ImapError::new("UID SEARCH", "search failed"),
    );
    let mut client = server.client();
    let result = create_draft(&mut client, &input(), true, NOW).await;
    assert!(result.is_err());
    assert_eq!(appends(&server), 0);
}

#[tokio::test]
async fn does_not_report_success_when_append_returns_false_or_verification_fails() {
    let rejected = server();
    rejected.lock().bool_override = Some(Box::new(|op, _| {
        (op == FakeOp::Append).then_some(Ok(false))
    }));
    let mut client = rejected.client();
    assert!(
        create_draft(&mut client, &input(), true, NOW)
            .await
            .is_err()
    );

    let invisible = server();
    // APPEND is acknowledged but the message never becomes visible.
    invisible.lock().bool_override =
        Some(Box::new(|op, _| (op == FakeOp::Append).then_some(Ok(true))));
    let mut client = invisible.client();
    let error = create_draft(&mut client, &input(), true, NOW)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("not yet visible"), "{error}");
}

#[tokio::test]
async fn reconciles_an_existing_draft_and_disallows_append_when_reconciliation_finds_nothing() {
    let message_id = deterministic_draft_message_id("draft-key");
    let existing = server();
    existing.put(
        "Drafts.localized",
        7,
        FakeMessage::new(format!("Message-ID: {message_id}\r\n\r\nx"))
            .envelope_id(Some(&message_id.to_uppercase())),
    );
    let mut client = existing.client();
    let result = create_draft(&mut client, &input(), false, NOW)
        .await
        .unwrap();
    assert_eq!(result.draft_id, message_id);
    assert!(result.already_existed);
    assert_eq!(appends(&existing), 0);

    let absent = server();
    let mut client = absent.client();
    assert!(
        create_draft(&mut client, &input(), false, NOW)
            .await
            .is_err()
    );
    assert_eq!(appends(&absent), 0);
}
