//! Link metadata extraction. Each fixture is split into header lines (folded
//! continuations joined) and the HTML or text body.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_core::email::{EmailLink, EmailLinkSource, ListUnsubscribe};
use omni_email::link_metadata::{HeaderLine, ParsedMailView, extract_email_link_metadata};

fn header_lines(headers: &[String]) -> Vec<HeaderLine> {
    let mut all: Vec<String> = vec![
        "From: Marketing <marketing@example.test>".to_owned(),
        "Message-ID: <fixture@example.test>".to_owned(),
    ];
    all.extend(headers.iter().cloned());
    let mut lines: Vec<HeaderLine> = Vec::new();
    for raw in all {
        if raw.starts_with([' ', '\t'])
            && let Some(last) = lines.last_mut()
        {
            last.line.push_str("\r\n");
            last.line.push_str(&raw);
            continue;
        }
        let key = raw.split(':').next().unwrap_or_default().to_lowercase();
        lines.push(HeaderLine { key, line: raw });
    }
    lines
}

fn metadata(body: &str, headers: &[String], html: bool) -> omni_core::email::EmailLinkMetadata {
    let content_type = format!(
        "Content-Type: text/{}; charset=utf-8",
        if html { "html" } else { "plain" }
    );
    let mut headers = headers.to_vec();
    headers.push(content_type);
    let lines = header_lines(&headers);
    extract_email_link_metadata(ParsedMailView {
        html: html.then_some(body),
        text: (!html).then_some(body),
        header_lines: &lines,
    })
}

fn html_metadata(body: &str) -> omni_core::email::EmailLinkMetadata {
    metadata(body, &[], true)
}

fn owned(headers: &[&str]) -> Vec<String> {
    headers.iter().map(|h| (*h).to_owned()).collect()
}

#[test]
fn caps_unrelated_header_scanning_and_reports_incomplete_header_evidence() {
    let mut headers: Vec<String> = (0..1001)
        .map(|_| "X-Unrelated: ignored".to_owned())
        .collect();
    headers.push("List-Unsubscribe: <https://example.test/unsubscribe>".to_owned());
    headers.push("List-Unsubscribe-Post: List-Unsubscribe=One-Click".to_owned());
    let result = metadata("hello", &headers, true);
    assert_eq!(
        result.list_unsubscribe,
        ListUnsubscribe {
            urls: vec![],
            post: None,
            present: false,
            truncated: true,
        }
    );
}

#[test]
fn decodes_inert_html_anchors_and_deduplicates_without_fetching_resources() {
    let result = html_metadata(
        r#"<img src="https://remote.test/pixel"><script>throw 1</script><a href="https://example.test/unsub?a=1&amp;b=2"> Unsubscribe &amp; preferences </a><a href="https://example.test/unsub?a=1&amp;b=2">again</a>"#,
    );
    assert_eq!(
        result.links,
        vec![EmailLink {
            url: "https://example.test/unsub?a=1&b=2".to_owned(),
            label: "Unsubscribe & preferences".to_owned(),
            source: EmailLinkSource::Html,
        }]
    );
    assert!(!result.links_truncated);
}

#[test]
fn rejects_unsafe_schemes_credentials_relative_and_malformed_urls() {
    let rejected = [
        "javascript:alert(1)",
        "data:text/html,hello",
        "/unsubscribe",
        "//example.test/path",
        "https://user:secret@example.test/",
        "https://",
        "https://exa mple.test/",
        "mailto:",
        "mailto:leave@example.test?subject=x%0d%0aBcc:other@example.test",
        "https://example.test/%00token",
        "https://example.test\\evil",
    ];
    let body: String = rejected
        .iter()
        .map(|url| format!("<a href=\"{url}\">Unsubscribe</a>"))
        .collect();
    assert!(html_metadata(&body).links.is_empty());
}

#[test]
fn extracts_text_urls_with_no_fabricated_label() {
    let result = metadata(
        "Use https://example.test/unsubscribe?token=opaque or mailto:leave@example.test",
        &[],
        false,
    );
    assert_eq!(
        result.links,
        vec![
            EmailLink {
                url: "https://example.test/unsubscribe?token=opaque".to_owned(),
                label: String::new(),
                source: EmailLinkSource::Text,
            },
            EmailLink {
                url: "mailto:leave@example.test".to_owned(),
                label: String::new(),
                source: EmailLinkSource::Text,
            },
        ]
    );
}

fn shop_links() -> String {
    (0..55)
        .map(|i| format!("<a href=\"https://example.test/{i}\">Shop</a>"))
        .collect()
}

#[test]
fn prioritizes_subscription_links_and_reports_bounded_omissions() {
    let body = format!(
        "{}<a href=\"https://example.test/unsubscribe\">Unsubscribe</a><a href=\"https://example.test/long\">{}</a><a href=\"https://example.test/{}\">too long</a>",
        shop_links(),
        "x".repeat(201),
        "x".repeat(4096)
    );
    let result = html_metadata(&body);
    assert_eq!(result.links.len(), 50);
    assert_eq!(result.links[0].label, "Unsubscribe");
    assert!(result.links_truncated);
    assert!(result.links.iter().all(|link| link.url.len() <= 4096));
}

#[test]
fn upgrades_duplicate_link_labels_before_applying_the_cap() {
    let body = format!(
        "{}<a href=\"https://example.test/54\">Manage preferences</a>",
        shop_links()
    );
    let result = html_metadata(&body);
    assert_eq!(
        result.links[0],
        EmailLink {
            url: "https://example.test/54".to_owned(),
            label: "Manage preferences".to_owned(),
            source: EmailLinkSource::Html,
        }
    );
}

#[test]
fn does_not_expose_partial_urls_at_scan_boundaries() {
    let body = format!(
        "{}<a href=\"https://example.test/unsubscribe?secret=abc\">Unsubscribe</a>",
        " ".repeat(1024 * 1024 - 20)
    );
    let result = html_metadata(&body);
    assert!(result.links.is_empty());
    assert!(result.links_truncated);
}

#[test]
fn reads_only_unsubscribe_headers_with_folding_and_repeats() {
    let result = metadata(
        "hello",
        &owned(&[
            "X-Private-Token: secret",
            "List-Unsubscribe: <https://example.test/unsubscribe?token=opaque>,",
            " <mailto:leave@example.test?subject=unsubscribe>",
            "List-Unsubscribe: <https://example.test/unsubscribe?token=opaque>",
            "List-Unsubscribe-Post: List-Unsubscribe=One-Click",
        ]),
        true,
    );
    assert_eq!(
        result.list_unsubscribe,
        ListUnsubscribe {
            urls: vec![
                "https://example.test/unsubscribe?token=opaque".to_owned(),
                "mailto:leave@example.test?subject=unsubscribe".to_owned(),
            ],
            post: Some("List-Unsubscribe=One-Click".to_owned()),
            present: true,
            truncated: false,
        }
    );
    assert!(!serde_json::to_string(&result).unwrap().contains("secret"));
}

#[test]
fn requires_an_unambiguous_exact_one_click_value() {
    for headers in [
        owned(&["List-Unsubscribe-Post: list-unsubscribe=one-click"]),
        owned(&["List-Unsubscribe-Post: List-Unsubscribe=One-Click; extra=value"]),
        owned(&[
            "List-Unsubscribe-Post: List-Unsubscribe=One-Click",
            "List-Unsubscribe-Post: List-Unsubscribe=One-Click",
        ]),
    ] {
        assert_eq!(
            metadata("hello", &headers, true).list_unsubscribe.post,
            None
        );
    }
}

#[test]
fn bounds_header_urls_and_header_scans_without_shortening_url_tokens() {
    let many: Vec<String> = (0..11)
        .map(|i| format!("<https://example.test/{i}>"))
        .collect();
    let result = metadata(
        "hello",
        &[
            format!("List-Unsubscribe: {}", many.join(", ")),
            format!(
                "List-Unsubscribe: <https://example.test/{}>",
                "x".repeat(4096)
            ),
            format!("List-Unsubscribe: {}", "x".repeat(16384)),
        ],
        true,
    );
    assert_eq!(result.list_unsubscribe.urls.len(), 10);
    assert!(result.list_unsubscribe.truncated);
}

#[test]
fn reports_absence_without_raw_header_output() {
    assert_eq!(
        metadata("hello", &[], true).list_unsubscribe,
        ListUnsubscribe {
            urls: vec![],
            post: None,
            present: false,
            truncated: false,
        }
    );
}

#[test]
fn distinguishes_post_only_headers_and_suppresses_one_click_on_incomplete_headers() {
    let post_only = metadata(
        "hello",
        &owned(&["List-Unsubscribe-Post: List-Unsubscribe=One-Click"]),
        true,
    );
    assert!(!post_only.list_unsubscribe.present);
    let incomplete = metadata(
        "hello",
        &[
            "List-Unsubscribe-Post: List-Unsubscribe=One-Click".to_owned(),
            format!("List-Unsubscribe-Post: {}", "x".repeat(16384)),
        ],
        true,
    );
    assert_eq!(incomplete.list_unsubscribe.post, None);
    assert!(incomplete.list_unsubscribe.truncated);
}
