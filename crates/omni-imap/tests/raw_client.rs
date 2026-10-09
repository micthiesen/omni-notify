//! The raw imap-proto client against a scripted server, including the port of
//! `src/email/imap/uidExpunge.spec.ts` (a lone `UID EXPUNGE <uid>`, never a
//! mailbox-wide EXPUNGE). Also covers what async-imap 0.11.3 could not
//! provide: COPYUID from untagged (MOVE) and tagged (COPY) OK codes, plus
//! capability re-read after LOGIN, literals, IDLE and modified UTF-7 names.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use omni_imap::protocol::raw::RawClient;
use omni_imap::protocol::{FetchQuery, IdleEnd, ImapClient, SearchCriteria, SourceRange, UidSet};
use tokio::io::{
    AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader, DuplexStream,
};
use tokio::sync::Notify;

/// One scripted exchange: the expected command prefix (without tag, with
/// literals inlined) and the reply lines, where `{tag}` is the command's tag.
struct Step {
    expect: &'static str,
    reply: Vec<String>,
}

fn step(expect: &'static str, reply: &[&str]) -> Step {
    Step {
        expect,
        reply: reply.iter().map(|s| (*s).to_owned()).collect(),
    }
}

/// `{n}` / `{n+}` at the end of a line.
fn literal_size(line: &str) -> Option<(usize, bool)> {
    let open = line.rfind('{')?;
    let inner = line[open + 1..].strip_suffix('}')?;
    let (digits, plus) = match inner.strip_suffix('+') {
        Some(d) => (d, true),
        None => (inner, false),
    };
    digits.parse().ok().map(|n| (n, plus))
}

/// Serves `greeting` then the steps; records every command line.
fn serve(greeting: &str, steps: Vec<Step>) -> (DuplexStream, Arc<Mutex<Vec<String>>>) {
    let (client, server) = tokio::io::duplex(1 << 20);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let greeting = greeting.to_owned();
    tokio::spawn(async move {
        let (read, mut write) = tokio::io::split(server);
        let mut reader = BufReader::new(read);
        write
            .write_all(format!("{greeting}\r\n").as_bytes())
            .await
            .unwrap();
        for step in steps {
            let mut raw = Vec::new();
            if reader.read_until(b'\n', &mut raw).await.unwrap() == 0 {
                return;
            }
            let mut line = String::from_utf8_lossy(&raw).trim_end().to_owned();
            // Inline literals: answer the continuation, read the bytes and the rest.
            while let Some((size, plus)) = literal_size(&line) {
                if !plus {
                    write.write_all(b"+ go ahead\r\n").await.unwrap();
                }
                let mut body = vec![0u8; size];
                reader.read_exact(&mut body).await.unwrap();
                let mut rest = Vec::new();
                reader.read_until(b'\n', &mut rest).await.unwrap();
                line.push_str(&String::from_utf8_lossy(&body));
                line.push_str(String::from_utf8_lossy(&rest).trim_end());
            }
            log.lock().unwrap().push(line.clone());
            let (tag, command) = line.split_once(' ').unwrap();
            assert!(
                command.starts_with(step.expect),
                "expected {:?}, got {command:?}",
                step.expect
            );
            if step.expect == "IDLE" {
                write.write_all(b"+ idling\r\n").await.unwrap();
                let mut done = String::new();
                reader.read_line(&mut done).await.unwrap();
                log.lock().unwrap().push(done.trim_end().to_owned());
            }
            for reply in step.reply {
                write
                    .write_all(format!("{}\r\n", reply.replace("{tag}", tag)).as_bytes())
                    .await
                    .unwrap();
            }
        }
    });
    (client, seen)
}

async fn logged_in(
    steps: Vec<Step>,
    caps: &str,
) -> (RawClient<DuplexStream>, Arc<Mutex<Vec<String>>>) {
    let mut all = vec![
        step("LOGIN", &["{tag} OK LOGIN completed"]),
        Step {
            expect: "CAPABILITY",
            reply: vec![format!("* CAPABILITY {caps}"), "{tag} OK done".to_owned()],
        },
    ];
    all.extend(steps);
    let (stream, seen) = serve("* OK [CAPABILITY IMAP4rev1 AUTH=PLAIN] ready", all);
    let mut client = RawClient::greet(stream, Duration::from_secs(5))
        .await
        .unwrap();
    client
        .login("user@example.test", "app-password")
        .await
        .unwrap();
    (client, seen)
}

#[tokio::test]
async fn runs_through_imap_flows_idle_aware_dispatcher_and_sends_one_uid() {
    let (mut client, seen) = logged_in(
        vec![
            step(
                "SELECT",
                &[
                    "* 3 EXISTS",
                    "* OK [UIDVALIDITY 10] ok",
                    "{tag} OK [READ-WRITE] selected",
                ],
            ),
            step("UID EXPUNGE 37", &["* 2 EXPUNGE", "{tag} OK expunged"]),
        ],
        "IMAP4rev1 IDLE UIDPLUS MOVE",
    )
    .await;
    client.select("INBOX", false).await.unwrap();
    assert!(client.uid_expunge(37).await.unwrap());
    let commands = seen.lock().unwrap().clone();
    assert!(commands.last().unwrap().ends_with("UID EXPUNGE 37"));
    assert!(!commands.iter().any(|c| c.ends_with(" EXPUNGE")));
}

#[tokio::test]
async fn rejects_missing_uidplus_and_cleans_up_a_failed_wire_command() {
    let (mut client, seen) = logged_in(Vec::new(), "IMAP4rev1 IDLE").await;
    let error = client.uid_expunge(37).await.unwrap_err();
    assert!(error.to_string().contains("unavailable"), "{error}");
    assert_eq!(
        seen.lock().unwrap().len(),
        2,
        "no wire command after LOGIN/CAPABILITY"
    );

    // With UIDPLUS, a lost connection fails the command and the session.
    let (mut client, _) = logged_in(Vec::new(), "IMAP4rev1 UIDPLUS").await;
    let error = client.uid_expunge(37).await.unwrap_err();
    assert!(
        error.to_string().starts_with("UID EXPUNGE failed"),
        "{error}"
    );
    assert!(!client.usable());
}

#[tokio::test]
async fn capabilities_are_reread_after_login() {
    let (client, _) = logged_in(Vec::new(), "IMAP4rev1 IDLE MOVE UIDPLUS LITERAL+").await;
    assert!(client.has_capability("MOVE"));
    assert!(client.has_capability("UIDPLUS"));
    assert!(
        !client.has_capability("AUTH=PLAIN"),
        "pre-login capabilities are not trusted"
    );
}

#[tokio::test]
async fn surfaces_copyuid_for_move_and_copy() {
    let (mut client, _) = logged_in(
        vec![
            step("SELECT", &["* OK [UIDVALIDITY 10] ok", "{tag} OK selected"]),
            step(
                "UID MOVE 7 \"Archive\"",
                &[
                    "* OK [COPYUID 20 7 12] moved",
                    "* 1 EXPUNGE",
                    "{tag} OK Move completed",
                ],
            ),
            step(
                "UID COPY 8 \"Archive\"",
                &["{tag} OK [COPYUID 20 8 13] Copy completed"],
            ),
            step("UID MOVE 9 \"Archive\"", &["{tag} NO no such message"]),
        ],
        "IMAP4rev1 MOVE UIDPLUS",
    )
    .await;
    client.select("INBOX", false).await.unwrap();
    let moved = client.uid_move(7, "Archive").await.unwrap().unwrap();
    assert_eq!(
        (moved.uid_validity, moved.destination_of(7)),
        (20, Some(12))
    );
    let copied = client.uid_copy(8, "Archive").await.unwrap().unwrap();
    assert_eq!(
        (copied.uid_validity, copied.destination_of(8)),
        (20, Some(13))
    );
    assert_eq!(client.uid_move(9, "Archive").await.unwrap(), None);
}

#[tokio::test]
async fn caches_selection_and_fetches_literal_sources_with_peek() {
    let (mut client, seen) = logged_in(
        vec![
            step("EXAMINE \"INBOX\"", &["* 1 EXISTS", "* OK [UIDVALIDITY 42] ok", "{tag} OK [READ-ONLY] examined"]),
            step(
                "UID FETCH 7 (UID FLAGS INTERNALDATE ENVELOPE BODY.PEEK[]<0.100>)",
                &[
                    "* 1 FETCH (UID 7 FLAGS (\\Seen) INTERNALDATE \"30-Sep-2026 12:00:00 +0000\" ENVELOPE (NIL NIL NIL NIL NIL NIL NIL NIL NIL \"<a@b>\") BODY[]<0> {5}",
                    "hello)",
                    "{tag} OK fetched",
                ],
            ),
        ],
        "IMAP4rev1",
    )
    .await;
    client.select("INBOX", true).await.unwrap();
    client.select("INBOX", true).await.unwrap();
    assert_eq!(client.selected().unwrap().uid_validity, "42");
    let query = FetchQuery {
        source: Some(SourceRange::Prefix { max_length: 100 }),
        internal_date: true,
        flags: true,
        envelope: true,
        size: false,
    };
    let fetched = client
        .uid_fetch(&UidSet::List(vec![7]), query)
        .await
        .unwrap();
    assert_eq!(fetched.len(), 1);
    assert_eq!(fetched[0].source.as_deref(), Some(&b"hello"[..]));
    assert_eq!(fetched[0].envelope_message_id.as_deref(), Some("<a@b>"));
    assert_eq!(fetched[0].internal_date_ms, Some(1_790_769_600_000));
    assert_eq!(
        fetched[0].flags.as_deref(),
        Some(&["\\Seen".to_owned()][..])
    );
    assert_eq!(
        seen.lock()
            .unwrap()
            .iter()
            .filter(|c| c.contains("EXAMINE"))
            .count(),
        1
    );
}

#[tokio::test]
async fn searches_unicode_with_charset_literals_and_appends_with_continuation() {
    let (mut client, seen) = logged_in(
        vec![
            step(
                "UID SEARCH CHARSET UTF-8 TEXT {5}caf\u{e9} UNSEEN SINCE 30-Sep-2026",
                &["* SEARCH 3 1 2", "{tag} OK searched"],
            ),
            step(
                "APPEND \"Sent Messages\" (\\Seen) \"30-Sep-2026 12:00:00 +0000\" {4}MIME",
                &["{tag} OK [APPENDUID 9 4] appended"],
            ),
        ],
        "IMAP4rev1",
    )
    .await;
    let criteria = SearchCriteria {
        text: Some("caf\u{e9}".to_owned()),
        seen: Some(false),
        since_ms: Some(1_790_769_600_000),
        ..SearchCriteria::default()
    };
    let found = client.uid_search(&criteria).await.unwrap();
    assert_eq!(found, Some(vec![1, 2, 3]));
    assert!(
        client
            .append(
                "Sent Messages",
                b"MIME",
                &["\\Seen"],
                Some(1_790_769_600_000)
            )
            .await
            .unwrap()
    );
    assert!(seen.lock().unwrap().iter().any(|c| c.contains("APPEND")));
}

#[tokio::test]
async fn lists_special_use_and_decodes_modified_utf7() {
    let (mut client, _) = logged_in(
        vec![step(
            "LIST \"\" \"*\" RETURN (SPECIAL-USE)",
            &[
                "* LIST (\\HasNoChildren) \"/\" \"INBOX\"",
                "* LIST (\\HasNoChildren \\Sent) \"/\" \"Courrier envoy&AOk-\"",
                "* LIST (\\HasNoChildren \\Archive) \"/\" \"Archive\"",
                "* LIST (\\HasNoChildren) \"/\" \"Drafts\"",
                "* WEIRD response imap-proto cannot parse",
                "{tag} OK listed",
            ],
        )],
        "IMAP4rev1 SPECIAL-USE",
    )
    .await;
    let list = client.list().await.unwrap();
    let role = |path: &str| {
        list.iter()
            .find(|m| m.path == path)
            .and_then(|m| m.special_use.clone())
    };
    assert_eq!(role("Courrier envoy\u{e9}").as_deref(), Some("\\Sent"));
    assert_eq!(role("Archive").as_deref(), Some("\\Archive"));
    assert_eq!(role("Drafts").as_deref(), Some("\\Drafts"));
    assert_eq!(role("INBOX").as_deref(), Some("\\Inbox"));
}

#[tokio::test]
async fn idle_ends_on_interrupt_with_done() {
    let (mut client, seen) = logged_in(
        vec![
            step(
                "EXAMINE",
                &["* 1 EXISTS", "* OK [UIDVALIDITY 1] ok", "{tag} OK examined"],
            ),
            step("IDLE", &["{tag} OK idle done"]),
        ],
        "IMAP4rev1 IDLE",
    )
    .await;
    client.select("INBOX", true).await.unwrap();
    let interrupt = Notify::new();
    interrupt.notify_one();
    let end = client
        .idle(&interrupt, Duration::from_secs(60))
        .await
        .unwrap();
    assert_eq!(end, IdleEnd::Interrupted);
    assert!(seen.lock().unwrap().iter().any(|c| c == "DONE"));
}

#[tokio::test]
async fn lists_with_xlist_or_plain_list_like_imapflow() {
    // XLIST (without SPECIAL-USE): its rows parse as LIST rows and its role
    // flags are trusted.
    let (mut client, _) = logged_in(
        vec![step(
            "XLIST \"\" \"*\"",
            &[
                "* XLIST (\\HasNoChildren \\Inbox) \"/\" \"INBOX\"",
                "* XLIST (\\HasNoChildren \\Sent) \"/\" \"Outbox Copies\"",
                "{tag} OK listed",
            ],
        )],
        "IMAP4rev1 XLIST",
    )
    .await;
    let list = client.list().await.unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[1].special_use.as_deref(), Some("\\Sent"));

    // SPECIAL-USE with a server that rejects RETURN: retried as plain LIST.
    let (mut client, seen) = logged_in(
        vec![
            step(
                "LIST \"\" \"*\" RETURN (SPECIAL-USE)",
                &["{tag} BAD unknown RETURN option"],
            ),
            step(
                "LIST \"\" \"*\"",
                &[
                    "* LIST (\\HasNoChildren \\Sent) \"/\" \"Outbox Copies\"",
                    "{tag} OK listed",
                ],
            ),
        ],
        "IMAP4rev1 SPECIAL-USE",
    )
    .await;
    let list = client.list().await.unwrap();
    assert_eq!(list[0].special_use.as_deref(), Some("\\Sent"));
    assert_eq!(seen.lock().unwrap().len(), 4);

    // Neither extension: role flags are ignored and names decide.
    let (mut client, _) = logged_in(
        vec![step(
            "LIST \"\" \"*\"",
            &[
                "* LIST (\\HasNoChildren \\Sent) \"/\" \"Outbox Copies\"",
                "* LIST (\\HasNoChildren) \"/\" \"Sent Messages\"",
                "{tag} OK listed",
            ],
        )],
        "IMAP4rev1",
    )
    .await;
    let list = client.list().await.unwrap();
    assert_eq!(list[0].special_use, None);
    assert_eq!(list[1].special_use.as_deref(), Some("\\Sent"));
}

#[tokio::test]
async fn searches_dates_with_within_when_advertised() {
    let (mut client, seen) = logged_in(
        vec![step(
            "UID SEARCH UNSEEN YOUNGER ",
            &["* SEARCH 4", "{tag} OK searched"],
        )],
        "IMAP4rev1 WITHIN",
    )
    .await;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let criteria = SearchCriteria {
        seen: Some(false),
        since_ms: Some(now - 86_400_000),
        ..SearchCriteria::default()
    };
    assert_eq!(client.uid_search(&criteria).await.unwrap(), Some(vec![4]));
    let line = seen.lock().unwrap().last().unwrap().clone();
    let seconds: i64 = line.rsplit(' ').next().unwrap().parse().unwrap();
    assert!((86_400..86_460).contains(&seconds), "{line}");
    assert_eq!(omni_imap::protocol::raw::within_seconds(10_499, 0), 10);
    assert_eq!(omni_imap::protocol::raw::within_seconds(10_500, 0), 11);
    assert_eq!(omni_imap::protocol::raw::within_seconds(0, 10_000), 0);
}
