//! Port of `src/email/imap/sent.spec.ts` (Sent copy reconciliation).
//!
//! Lock release counts have no Rust equivalent (selection is exclusive
//! `&mut` access); the cases assert the selections and writes instead.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use futures::FutureExt as _;
use omni_imap::fake::{FakeCall, FakeMessage, FakeOp, FakeServer};
use omni_imap::ops::sent::{BeforeAppend, SentCopyInput, append_sent_copy, find_sent_copy};
use omni_imap::protocol::{FetchQuery, ImapError, SourceRange, UidSet};

const NOW: i64 = 1_790_812_750_000;
const MESSAGE_ID: &str = "<Original@example.test>";
/// 2026-09-30T23:59:10Z
const INTERNAL_DATE: i64 = 1_790_812_750_000;

fn content() -> String {
    [
        "From: Michael <me@example.test>",
        "To: clinic@example.test",
        "Cc: copy@example.test",
        "Bcc: private@example.test",
        "Date: Wed, 30 Sep 2026 23:59:10 +0000",
        &format!("Message-ID: {MESSAGE_ID}"),
        "In-Reply-To: <parent@example.test>",
        "References: <root@example.test> <parent@example.test>",
        "Subject: Re: Follow-up",
        "Content-Type: text/plain; charset=utf-8",
        "",
        "Hi,",
        "",
        "Please book the follow-up.",
    ]
    .join("\r\n")
}

fn input() -> SentCopyInput {
    SentCopyInput {
        message_id: MESSAGE_ID.to_owned(),
        content: content().into_bytes(),
        internal_date_ms: Some(INTERNAL_DATE),
    }
}

fn server() -> FakeServer {
    let server = FakeServer::default();
    server.folder("Sent Messages", 9, Some("\\Sent"));
    server
}

fn row(source: &str, envelope: &str) -> FakeMessage {
    FakeMessage::new(source.as_bytes().to_vec()).envelope_id(Some(envelope))
}

fn appends(server: &FakeServer) -> usize {
    server.count(|c| matches!(c, FakeCall::Append { .. }))
}

#[tokio::test]
async fn discovers_sent_appends_the_unchanged_mime_with_original_internaldate_and_verifies_it() {
    let server = server();
    let mut client = server.client();
    let saved = append_sent_copy(&mut client, &input(), true, None, NOW)
        .await
        .unwrap();
    assert_eq!(saved.message_id, MESSAGE_ID);
    assert_eq!(saved.mailbox, "Sent Messages");
    assert!(!saved.already_existed);
    assert!(server.calls().contains(&FakeCall::Append {
        path: "Sent Messages".to_owned(),
        flags: vec!["\\Seen".to_owned()],
        internal_date_ms: Some(INTERNAL_DATE),
    }));
    assert_eq!(appends(&server), 1);
    assert_eq!(
        server.message("Sent Messages", 1).unwrap().source,
        content().into_bytes()
    );
    assert!(server.calls().contains(&FakeCall::Fetch {
        folder: "Sent Messages".to_owned(),
        uids: UidSet::List(vec![1]),
        query: FetchQuery {
            envelope: true,
            source: Some(SourceRange::Full),
            ..FetchQuery::default()
        },
    }));
}

#[tokio::test]
async fn returns_an_exact_preexisting_copy_without_appending() {
    let server = server();
    server.put("Sent Messages", 4, row(&content(), MESSAGE_ID));
    let mut client = server.client();
    let saved = append_sent_copy(&mut client, &input(), true, None, NOW)
        .await
        .unwrap();
    assert!(saved.already_existed);
    assert_eq!(appends(&server), 0);
}

#[tokio::test]
async fn does_a_read_only_lookup_and_ignores_substring_or_case_insensitive_matches() {
    let server = server();
    server.put(
        "Sent Messages",
        2,
        row(&content(), &MESSAGE_ID.to_lowercase()),
    );
    server.put(
        "Sent Messages",
        3,
        row(&content(), &format!("{MESSAGE_ID} other")),
    );
    let mut client = server.client();
    assert_eq!(
        find_sent_copy(&mut client, MESSAGE_ID, NOW).await.unwrap(),
        None
    );
    server.put("Sent Messages", 4, row(&content(), MESSAGE_ID));
    let found = find_sent_copy(&mut client, MESSAGE_ID, NOW)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.mailbox, "Sent Messages");
    assert!(found.already_existed);
    assert!(server.calls().contains(&FakeCall::Select {
        path: "Sent Messages".to_owned(),
        read_only: true
    }));
    assert_eq!(appends(&server), 0);
}

#[tokio::test]
async fn rejects_an_existing_message_id_with_changed_content() {
    let cases = [
        ("body", "Please book the follow-up.", "Different body"),
        ("from", "me@example.test", "someone@example.test"),
        ("sender name", "From: Michael", "From: Someone"),
        ("to", "clinic@example.test", "different@example.test"),
        ("cc", "copy@example.test", "othercopy@example.test"),
        ("bcc", "private@example.test", "otherprivate@example.test"),
        ("date", "23:59:10", "23:59:11"),
        ("subject", "Re: Follow-up", "Changed subject"),
        (
            "reply",
            "In-Reply-To: <parent@example.test>",
            "In-Reply-To: <other@example.test>",
        ),
        (
            "references",
            "<root@example.test>",
            "<otherroot@example.test>",
        ),
    ];
    for (kind, original, changed) in cases {
        let server = server();
        server.put(
            "Sent Messages",
            1,
            row(&content().replace(original, changed), MESSAGE_ID),
        );
        let mut client = server.client();
        let outcome = append_sent_copy(&mut client, &input(), true, None, NOW).await;
        assert!(outcome.is_err(), "changed {kind} was accepted");
        assert_eq!(appends(&server), 0, "{kind}");
    }
}

#[tokio::test]
async fn rejects_corruption_returned_after_append_and_releases_the_mailbox_lock() {
    let server = server();
    server.lock().bool_override = Some(Box::new(move |op, state| {
        if op != FakeOp::Append {
            return None;
        }
        let folder = state.folder_mut("Sent Messages").unwrap();
        folder.messages.insert(
            1,
            FakeMessage::new(content().replace("Follow-up", "Wrong").into_bytes())
                .envelope_id(Some(MESSAGE_ID)),
        );
        Some(Ok(true))
    }));
    let mut client = server.client();
    let outcome = append_sent_copy(&mut client, &input(), true, None, NOW).await;
    assert!(outcome.is_err());
    assert_eq!(appends(&server), 1);
}

#[tokio::test]
async fn accepts_semantically_identical_mime_with_different_line_endings() {
    let server = server();
    server.put(
        "Sent Messages",
        1,
        row(&content().replace("\r\n", "\n"), MESSAGE_ID),
    );
    let mut client = server.client();
    let saved = append_sent_copy(&mut client, &input(), true, None, NOW)
        .await
        .unwrap();
    assert!(saved.already_existed);
}

#[tokio::test]
async fn refuses_missing_or_ambiguous_server_designated_sent_folders() {
    for folders in [vec![], vec!["One", "Two"]] {
        let server = FakeServer::default();
        for (i, path) in folders.iter().enumerate() {
            server.folder(path, u32::try_from(i).unwrap() + 1, Some("\\Sent"));
        }
        let mut client = server.client();
        assert!(find_sent_copy(&mut client, MESSAGE_ID, NOW).await.is_err());
        assert_eq!(server.count(|c| matches!(c, FakeCall::Select { .. })), 0);
        assert_eq!(appends(&server), 0);
    }
}

#[tokio::test]
async fn refuses_candidate_overflow_rather_than_risking_a_duplicate_append() {
    let server = server();
    server.lock().search_override = Some(Box::new(|_, _| Some(Ok(Some((1..=51).collect())))));
    let mut client = server.client();
    assert!(
        append_sent_copy(&mut client, &input(), true, None, NOW)
            .await
            .is_err()
    );
    assert_eq!(server.count(|c| matches!(c, FakeCall::Fetch { .. })), 0);
    assert_eq!(appends(&server), 0);
}

#[tokio::test]
async fn reconciles_an_uncertain_append_without_issuing_another_write() {
    let server = server();
    let mut client = server.client();
    assert!(
        append_sent_copy(&mut client, &input(), false, None, NOW)
            .await
            .is_err()
    );
    assert_eq!(appends(&server), 0);
    server.put("Sent Messages", 1, row(&content(), MESSAGE_ID));
    let saved = append_sent_copy(&mut client, &input(), false, None, NOW)
        .await
        .unwrap();
    assert!(saved.already_existed);
    assert!(server.calls().contains(&FakeCall::Select {
        path: "Sent Messages".to_owned(),
        read_only: true
    }));
    assert_eq!(appends(&server), 0);
}

#[tokio::test]
async fn never_repeats_append_after_an_uncertain_outcome() {
    for kind in ["throw", "false", "invisible"] {
        let server = server();
        match kind {
            "throw" => server.fail_next(
                FakeOp::Append,
                None,
                ImapError::new("APPEND", "connection lost"),
            ),
            "false" => {
                server.lock().bool_override = Some(Box::new(|op, _| {
                    (op == FakeOp::Append).then_some(Ok(false))
                }))
            }
            _ => {
                server.lock().bool_override =
                    Some(Box::new(|op, _| (op == FakeOp::Append).then_some(Ok(true))))
            }
        }
        let mut client = server.client();
        assert!(
            append_sent_copy(&mut client, &input(), true, None, NOW)
                .await
                .is_err(),
            "{kind}"
        );
        assert_eq!(appends(&server), 1, "{kind}");
    }
}

#[tokio::test]
async fn rejects_missing_authoritative_headers_or_an_invalid_original_date_before_mailbox_access() {
    let server = server();
    let mut client = server.client();
    let invalid = [
        SentCopyInput {
            message_id: "<different@example.test>".to_owned(),
            ..input()
        },
        SentCopyInput {
            internal_date_ms: None,
            ..input()
        },
        SentCopyInput {
            content: format!("Message-ID: {MESSAGE_ID}\r\n\r\nBody").into_bytes(),
            ..input()
        },
    ];
    for candidate in invalid {
        assert!(
            append_sent_copy(&mut client, &candidate, true, None, NOW)
                .await
                .is_err()
        );
    }
    assert_eq!(server.count(|c| matches!(c, FakeCall::List)), 0);
    assert_eq!(appends(&server), 0);
}

#[tokio::test]
async fn claims_append_only_after_lookup_and_never_appends_without_the_durable_permit() {
    let server = server();
    let mut client = server.client();
    let before: BeforeAppend = Box::new(|| async { Ok(false) }.boxed());
    assert!(
        append_sent_copy(&mut client, &input(), true, Some(before), NOW)
            .await
            .is_err()
    );
    assert_eq!(appends(&server), 0);
}
