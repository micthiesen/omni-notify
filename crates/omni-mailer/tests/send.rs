//! SMTP sending against an in-process fake SMTP server. A partial RCPT
//! rejection is an error, never success. Missing SMTP configuration (no
//! `Mailer`) is covered in `client.rs`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};

use omni_http::SideEffectMode;
use omni_mailer::{ComposeInput, MailError, Mailer};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::TcpListener;

#[derive(Clone, Debug, Default)]
struct Transaction {
    mail_from: String,
    rcpt_to: Vec<String>,
    data: String,
}

#[derive(Clone, Default)]
struct FakeSmtp {
    transactions: Arc<Mutex<Vec<Transaction>>>,
    connections: Arc<Mutex<usize>>,
}

impl FakeSmtp {
    /// Starts a server that rejects `RCPT TO` for `reject`.
    async fn start(reject: &'static [&'static str]) -> (Self, u16) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = Self::default();
        let state = server.clone();
        tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    return;
                };
                *state.connections.lock().unwrap() += 1;
                let state = state.clone();
                tokio::spawn(async move {
                    let (read, mut write) = socket.into_split();
                    let mut lines = BufReader::new(read).lines();
                    write.write_all(b"220 fake ESMTP\r\n").await.unwrap();
                    let mut current = Transaction::default();
                    while let Ok(Some(line)) = lines.next_line().await {
                        let upper = line.to_ascii_uppercase();
                        let reply: String =
                            if upper.starts_with("EHLO") || upper.starts_with("HELO") {
                                "250 fake\r\n".to_owned()
                            } else if upper.starts_with("MAIL FROM:") {
                                current = Transaction {
                                    mail_from: angle(&line),
                                    ..Transaction::default()
                                };
                                "250 OK\r\n".to_owned()
                            } else if upper.starts_with("RCPT TO:") {
                                let rcpt = angle(&line);
                                if reject.contains(&rcpt.as_str()) {
                                    format!("550 5.1.1 <{rcpt}> rejected\r\n")
                                } else {
                                    current.rcpt_to.push(rcpt);
                                    "250 OK\r\n".to_owned()
                                }
                            } else if upper == "DATA" {
                                write.write_all(b"354 go\r\n").await.unwrap();
                                let mut data = String::new();
                                while let Ok(Some(body)) = lines.next_line().await {
                                    if body == "." {
                                        break;
                                    }
                                    data.push_str(&body);
                                    data.push('\n');
                                }
                                current.data = data;
                                state.transactions.lock().unwrap().push(current.clone());
                                "250 queued\r\n".to_owned()
                            } else if upper == "QUIT" {
                                let _ = write.write_all(b"221 bye\r\n").await;
                                return;
                            } else {
                                "250 OK\r\n".to_owned()
                            };
                        if write.write_all(reply.as_bytes()).await.is_err() {
                            return;
                        }
                    }
                });
            }
        });
        (server, port)
    }

    fn transactions(&self) -> Vec<Transaction> {
        self.transactions.lock().unwrap().clone()
    }

    fn connections(&self) -> usize {
        *self.connections.lock().unwrap()
    }
}

fn angle(line: &str) -> String {
    let start = line.find('<').map_or(0, |i| i + 1);
    let end = line[start..].find('>').map_or(line.len(), |i| start + i);
    line[start..end].to_owned()
}

fn mailer(port: u16) -> Mailer {
    Mailer::plaintext_for_tests("127.0.0.1", port, SideEffectMode::Live)
}

fn input(to: &[&str], cc: &[&str], bcc: &[&str]) -> ComposeInput {
    let owned = |list: &[&str]| list.iter().map(|s| (*s).to_owned()).collect();
    ComposeInput {
        to: owned(to),
        cc: owned(cc),
        bcc: owned(bcc),
        subject: "Test".to_owned(),
        text: "Body".to_owned(),
        ..ComposeInput::default()
    }
}

#[tokio::test]
async fn sends_the_stable_message_id_and_succeeds_only_when_every_recipient_is_accepted() {
    let (server, port) = FakeSmtp::start(&[]).await;
    let report = mailer(port)
        .send_composed(
            &input(
                &["to@example.test"],
                &["cc@example.test"],
                &["bcc@example.test"],
            ),
            "<stable@omni-notify>",
            0,
        )
        .await
        .unwrap();
    assert_eq!(
        report.accepted,
        vec!["to@example.test", "cc@example.test", "bcc@example.test"]
    );
    assert!(report.rejected.is_empty());
    let sent = server.transactions();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].mail_from, "michael@thiesen.dev");
    assert!(sent[0].data.contains("Message-ID: <stable@omni-notify>"));
    assert!(
        sent[0].data.contains("From: <michael@thiesen.dev>")
            || sent[0].data.contains("From: michael@thiesen.dev")
    );
    assert!(
        !sent[0].data.contains("bcc@example.test"),
        "Bcc leaked into the wire MIME"
    );
}

#[tokio::test]
async fn fails_when_smtp_partially_rejects_recipients() {
    let (server, port) = FakeSmtp::start(&["second@example.test"]).await;
    let result = mailer(port)
        .send_composed(
            &input(&["to@example.test", "second@example.test"], &[], &[]),
            "<p@omni-notify>",
            0,
        )
        .await;
    assert!(matches!(result, Err(MailError::Smtp { .. })), "{result:?}");
    assert!(server.transactions().is_empty());
}

#[tokio::test]
async fn does_not_require_duplicate_to_and_cc_addresses_to_be_accepted_twice() {
    let (server, port) = FakeSmtp::start(&[]).await;
    let report = mailer(port)
        .send_composed(
            &input(&["same@example.test"], &["same@example.test"], &[]),
            "<d@omni-notify>",
            0,
        )
        .await
        .unwrap();
    assert_eq!(report.accepted, vec!["same@example.test"]);
    assert_eq!(server.transactions()[0].rcpt_to, vec!["same@example.test"]);
}

#[tokio::test]
async fn sends_persisted_wire_bytes_with_an_explicit_deduplicated_bcc_envelope() {
    let (server, port) = FakeSmtp::start(&[]).await;
    let raw = b"From: michael@thiesen.dev\r\nMessage-ID: <wire@test>\r\n\r\nBody\r\n";
    let recipients: Vec<String> = ["to@test", "cc@test", "to@test", "hidden@test"]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    let report = mailer(port).send_raw(&recipients, raw).await.unwrap();
    assert_eq!(report.accepted, vec!["to@test", "cc@test", "hidden@test"]);
    let sent = server.transactions();
    assert_eq!(sent[0].mail_from, "michael@thiesen.dev");
    assert_eq!(sent[0].rcpt_to, vec!["to@test", "cc@test", "hidden@test"]);
    assert!(sent[0].data.contains("Message-ID: <wire@test>"));
}

#[tokio::test]
async fn refuses_persisted_mime_with_a_different_from_before_contacting_smtp() {
    let (server, port) = FakeSmtp::start(&[]).await;
    let mailer = mailer(port);
    for raw in [
        &b"From: micthiesen@icloud.com\r\n\r\nBody"[..],
        &b"From: michael@thiesen.dev, other@example.test\r\n\r\nBody"[..],
        &b"From: michael@thiesen.dev\r\nSender: other@example.test\r\n\r\nBody"[..],
        &b"From: michael@thiesen.dev\r\nResent-From: other@example.test\r\n\r\nBody"[..],
    ] {
        let result = mailer.send_raw(&["to@example.test".to_owned()], raw).await;
        assert!(matches!(result, Err(MailError::Identity(_))), "{result:?}");
    }
    assert_eq!(server.connections(), 0);
}

#[tokio::test]
async fn uses_the_fixed_identity_for_notification_mail() {
    let (server, port) = FakeSmtp::start(&[]).await;
    mailer(port)
        .send_notification("to@example.test", "Logs", "<p>Body</p>", "Body")
        .await
        .unwrap();
    let sent = server.transactions();
    assert_eq!(sent[0].mail_from, "michael@thiesen.dev");
    assert_eq!(sent[0].rcpt_to, vec!["to@example.test"]);
    assert!(sent[0].data.contains("michael@thiesen.dev"));
    assert!(sent[0].data.contains("text/html"));
}

#[tokio::test]
async fn record_mode_never_contacts_smtp() {
    let (server, port) = FakeSmtp::start(&[]).await;
    let recording = Mailer::plaintext_for_tests("127.0.0.1", port, SideEffectMode::Record);
    recording
        .send_composed(&input(&["to@example.test"], &[], &[]), "<r@omni-notify>", 0)
        .await
        .unwrap();
    assert_eq!(server.connections(), 0);
    let recorded = recording.recorded();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].0, vec!["to@example.test"]);
}
