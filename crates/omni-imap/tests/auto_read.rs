//! Auto-read marking. Selection is exclusive `&mut` access, so cases assert
//! the writable selection, the outcome and the warning.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashSet;

use futures::FutureExt as _;
use omni_imap::fake::{FakeCall, FakeMessage, FakeOp, FakeServer};
use omni_imap::ops::auto_read::{
    AutoReadProtection, ProtectionFn, mark_recent_unread_read, select_auto_read_folders,
};
use omni_imap::protocol::{ImapError, MailboxInfo, SearchCriteria};

/// 2026-08-21T18:30:00Z
const NOW: i64 = 1_787_337_000_000;

fn mailbox(path: &str, special_use: Option<&str>, flags: &[&str]) -> MailboxInfo {
    MailboxInfo {
        path: path.to_owned(),
        flags: flags.iter().map(|f| (*f).to_owned()).collect(),
        special_use: special_use.map(str::to_owned),
    }
}

fn stores(server: &FakeServer) -> Vec<(String, Vec<u32>, bool)> {
    server
        .calls()
        .into_iter()
        .filter_map(|c| match c {
            FakeCall::Store {
                folder,
                uids,
                flags,
                silent,
            } => {
                assert_eq!(flags, vec!["\\Seen".to_owned()]);
                Some((folder, uids, silent))
            }
            _ => None,
        })
        .collect()
}

fn searches(server: &FakeServer) -> usize {
    server.count(|c| matches!(c, FakeCall::Search { .. }))
}

fn unread_server(folders: &[(&str, u32)], unread: &[(&str, u32)]) -> FakeServer {
    let server = FakeServer::default();
    for (path, validity) in folders {
        server.folder(path, *validity, None);
    }
    for (path, uid) in unread {
        server.put(
            path,
            *uid,
            FakeMessage::new(format!("Message-ID: <{uid}@x>\r\n\r\n")).date(NOW - 60_000),
        );
    }
    server
}

fn protection(
    f: impl Fn(String, Option<String>) -> AutoReadProtection + Send + Sync + 'static,
) -> Box<ProtectionFn> {
    Box::new(move |folder, validity| {
        let value = f(folder, validity);
        async move { Ok(value) }.boxed()
    })
}

fn warnings(capture: &omni_testkit::LogCapture) -> Vec<String> {
    capture
        .events()
        .into_iter()
        .filter(|e| e.level == tracing::Level::WARN)
        .map(|e| e.message)
        .collect()
}

#[test]
fn selects_localized_paths_by_exact_special_use_role() {
    assert_eq!(
        select_auto_read_folders(&[
            mailbox("Archiv", Some("\\Archive"), &[]),
            mailbox("Spam", Some("\\Junk"), &[]),
            mailbox("Papierkorb", Some("\\Trash"), &[]),
        ]),
        vec!["Archiv", "Spam", "Papierkorb"]
    );
}

#[test]
fn ignores_other_roles_noselection_mailboxes_and_duplicate_paths() {
    assert_eq!(
        select_auto_read_folders(&[
            mailbox("All Mail", Some("\\All"), &[]),
            mailbox("Archive", Some("\\Archive"), &["\\Noselect"]),
            mailbox("Archive", Some("\\Archive"), &[]),
            mailbox("Archive", Some("\\Archive"), &[]),
            mailbox("Inbox", None, &[]),
        ]),
        vec!["Archive"]
    );
}

#[tokio::test]
async fn does_nothing_for_an_empty_unread_search() {
    let server = unread_server(&[("Archive", 20)], &[]);
    let mut client = server.client();
    mark_recent_unread_read(&mut client, &["Archive".to_owned()], NOW, None).await;
    assert!(stores(&server).is_empty());
    assert!(server.calls().contains(&FakeCall::Select {
        path: "Archive".to_owned(),
        read_only: false
    }));
}

#[tokio::test]
async fn searches_recent_unread_mail_and_adds_seen_by_uid_silently() {
    let server = unread_server(&[("Archive", 20)], &[("Archive", 4), ("Archive", 9)]);
    let mut client = server.client();
    mark_recent_unread_read(&mut client, &["Archive".to_owned()], NOW, None).await;
    assert!(server.calls().contains(&FakeCall::Search {
        folder: "Archive".to_owned(),
        criteria: SearchCriteria {
            seen: Some(false),
            since_ms: Some(NOW - 24 * 60 * 60_000),
            ..SearchCriteria::default()
        }
    }));
    assert_eq!(
        stores(&server),
        vec![("Archive".to_owned(), vec![4, 9], true)]
    );
}

#[tokio::test]
async fn leaves_protected_archive_uids_unread_while_marking_ordinary_mail() {
    let server = unread_server(
        &[("Archive", 20)],
        &[("Archive", 4), ("Archive", 9), ("Archive", 12)],
    );
    let mut client = server.client();
    let lookup = protection(|_, validity| {
        assert_eq!(validity.as_deref(), Some("20"));
        AutoReadProtection {
            skip: false,
            excluded_uids: HashSet::from([9, 12]),
            fallback_message_ids: Vec::new(),
        }
    });
    mark_recent_unread_read(
        &mut client,
        &["Archive".to_owned()],
        NOW,
        Some(lookup.as_ref()),
    )
    .await;
    assert_eq!(stores(&server), vec![("Archive".to_owned(), vec![4], true)]);
}

#[tokio::test]
async fn resolves_an_uncertain_action_by_exact_message_id_and_marks_other_archive_mail() {
    let protected = "<protected@example.test>";
    let server = unread_server(&[("Archive", 20)], &[("Archive", 4)]);
    server.put(
        "Archive",
        9,
        FakeMessage::new(format!("Message-ID: {protected}\r\n\r\n")).date(NOW),
    );
    server.put(
        "Archive",
        10,
        FakeMessage::new("Message-ID: <substring@example.test>\r\n\r\n".to_owned()).date(NOW),
    );
    server.lock().search_override = Some(Box::new(|_, criteria| {
        Some(Ok(Some(if criteria.header.is_some() {
            vec![9, 10]
        } else {
            vec![4, 9, 10]
        })))
    }));
    let mut client = server.client();
    let lookup = protection(move |_, _| AutoReadProtection {
        skip: false,
        excluded_uids: HashSet::new(),
        fallback_message_ids: vec![protected.to_owned()],
    });
    mark_recent_unread_read(
        &mut client,
        &["Archive".to_owned()],
        NOW,
        Some(lookup.as_ref()),
    )
    .await;
    assert_eq!(
        stores(&server),
        vec![("Archive".to_owned(), vec![4, 10], true)]
    );
}

#[tokio::test]
async fn leaves_archive_unread_for_this_pass_when_protected_identity_lookup_fails() {
    let capture = omni_testkit::capture_logs();
    let server = unread_server(&[("Archive", 20)], &[("Archive", 4), ("Archive", 9)]);
    server.lock().search_override = Some(Box::new(|_, criteria| {
        criteria.header.is_some().then_some(Ok(None))
    }));
    let mut client = server.client();
    let lookup = protection(|_, _| AutoReadProtection {
        skip: false,
        excluded_uids: HashSet::new(),
        fallback_message_ids: vec!["<protected@example.test>".to_owned()],
    });
    mark_recent_unread_read(
        &mut client,
        &["Archive".to_owned()],
        NOW,
        Some(lookup.as_ref()),
    )
    .await;
    assert!(stores(&server).is_empty());
    assert_eq!(warnings(&capture).len(), 1);
}

#[tokio::test]
async fn skips_ambiguous_archive_state_while_continuing_junk_cleanup() {
    let server = unread_server(
        &[("Archive", 20), ("Junk", 21)],
        &[("Archive", 4), ("Junk", 4)],
    );
    let mut client = server.client();
    let lookup = protection(|folder, _| AutoReadProtection {
        skip: folder == "Archive",
        ..AutoReadProtection::default()
    });
    mark_recent_unread_read(
        &mut client,
        &["Archive".to_owned(), "Junk".to_owned()],
        NOW,
        Some(lookup.as_ref()),
    )
    .await;
    assert_eq!(searches(&server), 1);
    assert_eq!(stores(&server).len(), 1);
}

#[tokio::test]
async fn releases_the_writable_lock_when_marking_succeeds_or_fails() {
    let capture = omni_testkit::capture_logs();
    let success = unread_server(&[("Archive", 20)], &[("Archive", 3)]);
    let mut client = success.client();
    mark_recent_unread_read(&mut client, &["Archive".to_owned()], NOW, None).await;
    assert_eq!(stores(&success).len(), 1);

    let failure = unread_server(&[("Archive", 20)], &[("Archive", 3)]);
    failure.fail_next(
        FakeOp::Search,
        None,
        ImapError::new("UID SEARCH", "search failed"),
    );
    let mut client = failure.client();
    mark_recent_unread_read(&mut client, &["Archive".to_owned()], NOW, None).await;
    assert!(failure.calls().contains(&FakeCall::Select {
        path: "Archive".to_owned(),
        read_only: false
    }));
    assert!(
        warnings(&capture)
            .iter()
            .any(|w| w.contains("folder \"Archive\""))
    );
}

#[tokio::test]
async fn continues_after_one_folder_fails() {
    let capture = omni_testkit::capture_logs();
    let server = unread_server(&[("Junk", 21), ("Trash", 22)], &[("Trash", 8)]);
    server.fail_next(
        FakeOp::Select,
        None,
        ImapError::new("SELECT Junk", "unavailable"),
    );
    let mut client = server.client();
    mark_recent_unread_read(
        &mut client,
        &["Junk".to_owned(), "Trash".to_owned()],
        NOW,
        None,
    )
    .await;
    assert_eq!(searches(&server), 1);
    assert_eq!(stores(&server), vec![("Trash".to_owned(), vec![8], true)]);
    assert!(
        warnings(&capture)
            .iter()
            .any(|w| w.contains("folder \"Junk\""))
    );
}

#[tokio::test]
async fn continues_after_a_selected_folder_search_fails() {
    let capture = omni_testkit::capture_logs();
    let server = unread_server(
        &[("Junk", 21), ("Trash", 22)],
        &[("Junk", 5), ("Trash", 12)],
    );
    server.fail_next(
        FakeOp::Search,
        Some("Junk"),
        ImapError::new("UID SEARCH", "search failed"),
    );
    let mut client = server.client();
    mark_recent_unread_read(
        &mut client,
        &["Junk".to_owned(), "Trash".to_owned()],
        NOW,
        None,
    )
    .await;
    assert_eq!(searches(&server), 2);
    assert_eq!(stores(&server), vec![("Trash".to_owned(), vec![12], true)]);
    assert!(
        warnings(&capture)
            .iter()
            .any(|w| w.contains("folder \"Junk\""))
    );
}
