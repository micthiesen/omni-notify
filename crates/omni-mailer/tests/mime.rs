//! Outgoing MIME rendering, parsed back with mail-parser.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use mail_parser::{HeaderValue, MessageParser};
use omni_mailer::{ComposeInput, OutgoingEmailAttachment, prepare_composed_email};

fn ids(value: &HeaderValue<'_>) -> Vec<String> {
    match value.as_text_list() {
        Some(list) => list.iter().map(ToString::to_string).collect(),
        None => value.as_text().map(str::to_owned).into_iter().collect(),
    }
}

#[test]
fn keeps_bcc_private_preserves_date_body_subject_and_extends_the_reply_chain() {
    let date_ms = jiff::Timestamp::from_second(1_790_812_745)
        .unwrap()
        .as_millisecond();
    assert_eq!(
        omni_core::js::to_iso_string(date_ms),
        "2026-09-30T23:59:05.000Z"
    );
    let prepared = prepare_composed_email(
        &ComposeInput {
            to: vec!["to@example.test".to_owned()],
            cc: vec!["cc@example.test".to_owned()],
            bcc: vec!["hidden@example.test".to_owned()],
            subject: "Re: Sam — follow-up".to_owned(),
            text: "Hi,\n\nExact body.\nMichael".to_owned(),
            in_reply_to: Some("<parent@example.test>".to_owned()),
            references: vec!["<root@example.test>".to_owned()],
            attachments: Vec::new(),
        },
        "<stable@example.test>",
        date_ms,
    )
    .unwrap();
    assert_eq!(prepared.from, "michael@thiesen.dev");
    assert_eq!(prepared.date_iso, "2026-09-30T23:59:05.000Z");
    assert_eq!(prepared.message_id, "<stable@example.test>");

    let wire_bytes = STANDARD.decode(&prepared.wire_b64).unwrap();
    let copy_bytes = STANDARD.decode(&prepared.content_b64).unwrap();
    let wire = MessageParser::default().parse(&wire_bytes).unwrap();
    let copy = MessageParser::default().parse(&copy_bytes).unwrap();
    assert!(wire.bcc().is_none());
    assert!(!String::from_utf8_lossy(&wire_bytes).contains("hidden@example.test"));
    let bcc: Vec<&str> = copy
        .bcc()
        .unwrap()
        .iter()
        .filter_map(|a| a.address())
        .collect();
    assert_eq!(bcc, vec!["hidden@example.test"]);

    for parsed in [&wire, &copy] {
        let from: Vec<_> = parsed
            .from()
            .unwrap()
            .iter()
            .map(|a| (a.address().map(str::to_owned), a.name().map(str::to_owned)))
            .collect();
        assert_eq!(from, vec![(Some("michael@thiesen.dev".to_owned()), None)]);
        assert_eq!(parsed.message_id(), Some("stable@example.test"));
        assert_eq!(parsed.date().unwrap().to_timestamp(), 1_790_812_745);
        assert_eq!(parsed.subject(), Some("Re: Sam — follow-up"));
        assert_eq!(ids(parsed.in_reply_to()), vec!["parent@example.test"]);
        assert_eq!(
            ids(parsed.references()),
            vec!["root@example.test", "parent@example.test"]
        );
        // Line endings are canonical CRLF on the wire; node mailparser
        // reports them as LF.
        assert_eq!(
            parsed
                .body_text(0)
                .unwrap()
                .replace("\r\n", "\n")
                .trim_end(),
            "Hi,\n\nExact body.\nMichael"
        );
        let to: Vec<&str> = parsed
            .to()
            .unwrap()
            .iter()
            .filter_map(|a| a.address())
            .collect();
        assert_eq!(to, vec!["to@example.test"]);
    }
}

#[test]
fn does_not_duplicate_a_parent_already_in_references() {
    let prepared = prepare_composed_email(
        &ComposeInput {
            to: vec!["to@example.test".to_owned()],
            subject: "Re: x".to_owned(),
            text: "x".to_owned(),
            in_reply_to: Some("<parent@example.test>".to_owned()),
            references: vec!["<parent@example.test>".to_owned()],
            ..ComposeInput::default()
        },
        "<id@example.test>",
        0,
    )
    .unwrap();
    let wire = STANDARD.decode(&prepared.wire_b64).unwrap();
    let parsed = MessageParser::default().parse(&wire).unwrap();
    assert_eq!(ids(parsed.references()), vec!["parent@example.test"]);
}

#[test]
fn adds_verified_pdfs_as_attachment_parts_to_both_the_wire_and_private_copy() {
    use mail_parser::MimeHeaders as _;
    let first = b"%PDF-1.7\nfirst synthetic fixture\n%%EOF".to_vec();
    let second = b"%PDF-1.4\nsecond synthetic fixture\n%%EOF".to_vec();
    let pdf = |filename: &str, content: &[u8]| OutgoingEmailAttachment {
        filename: filename.to_owned(),
        content_type: "application/pdf".to_owned(),
        content: content.to_vec(),
    };
    let date_ms = jiff::Timestamp::from_second(1_791_460_800)
        .unwrap()
        .as_millisecond();
    let prepared = prepare_composed_email(
        &ComposeInput {
            to: vec!["to@example.test".to_owned()],
            bcc: vec!["hidden@example.test".to_owned()],
            subject: "Re: Results".to_owned(),
            text: "Attached.\nMichael".to_owned(),
            in_reply_to: Some("<parent@example.test>".to_owned()),
            references: vec!["<root@example.test>".to_owned()],
            attachments: vec![
                pdf("Résumé \"final\".pdf", &first),
                pdf("scan.pdf", &second),
            ],
            ..ComposeInput::default()
        },
        "<attached@example.test>",
        date_ms,
    )
    .unwrap();
    let wire_bytes = STANDARD.decode(&prepared.wire_b64).unwrap();
    let copy_bytes = STANDARD.decode(&prepared.content_b64).unwrap();
    assert!(
        String::from_utf8_lossy(&wire_bytes)
            .lines()
            .any(|line| line.starts_with("Content-Type: multipart/mixed")),
        "wire MIME is not multipart/mixed"
    );
    let wire = MessageParser::default().parse(&wire_bytes).unwrap();
    let copy = MessageParser::default().parse(&copy_bytes).unwrap();
    assert!(wire.bcc().is_none());
    let bcc: Vec<&str> = copy
        .bcc()
        .unwrap()
        .iter()
        .filter_map(|a| a.address())
        .collect();
    assert_eq!(bcc, vec!["hidden@example.test"]);
    for parsed in [&wire, &copy] {
        assert_eq!(
            parsed
                .body_text(0)
                .unwrap()
                .replace("\r\n", "\n")
                .trim_end(),
            "Attached.\nMichael"
        );
        assert_eq!(ids(parsed.in_reply_to()), vec!["parent@example.test"]);
        assert_eq!(
            ids(parsed.references()),
            vec!["root@example.test", "parent@example.test"]
        );
        let parts: Vec<(String, String, String, Vec<u8>)> = parsed
            .attachments()
            .map(|part| {
                let content_type = part.content_type().unwrap();
                (
                    part.attachment_name().unwrap().to_owned(),
                    format!(
                        "{}/{}",
                        content_type.ctype(),
                        content_type.subtype().unwrap_or_default()
                    ),
                    part.content_disposition().unwrap().ctype().to_owned(),
                    part.contents().to_vec(),
                )
            })
            .collect();
        assert_eq!(
            parts,
            vec![
                (
                    "Résumé \"final\".pdf".to_owned(),
                    "application/pdf".to_owned(),
                    "attachment".to_owned(),
                    first.clone()
                ),
                (
                    "scan.pdf".to_owned(),
                    "application/pdf".to_owned(),
                    "attachment".to_owned(),
                    second.clone()
                ),
            ]
        );
    }
}
