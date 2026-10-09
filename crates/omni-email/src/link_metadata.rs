//! Inert, bounded link and List-Unsubscribe metadata.
//! URLs are parsed for validation only, never followed; no raw headers or
//! remote content escape. Lengths and slices use JS UTF-16 semantics.

use std::collections::HashMap;
use std::sync::LazyLock;

use omni_core::email::{EmailLink, EmailLinkMetadata, EmailLinkSource, ListUnsubscribe};
use omni_core::js::{utf16_len, utf16_slice};
use regex::Regex;
use scraper::{Html, Selector};

use crate::sender_rules::{is_js_space, js_trim};

const SCAN_LIMIT: usize = 1024 * 1024;
const URL_LIMIT: usize = 4096;
const HEADER_LIMIT: usize = 16 * 1024;
const HEADER_COUNT_LIMIT: usize = 1000;
const MAX_LINKS: usize = 50;
const MAX_LABEL: usize = 200;
const MAX_UNSUBSCRIBE_URLS: usize = 10;
const ONE_CLICK: &str = "List-Unsubscribe=One-Click";

#[allow(clippy::expect_used)]
static PREFERRED_LABEL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)unsubscribe|opt[ -]?out|preferences|manage (?:email|subscription)")
        .expect("static regex")
});
#[allow(clippy::expect_used)]
static UNSAFE_CHARS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\x00-\x20\x7f\\]").expect("static regex"));
#[allow(clippy::expect_used)]
static UNSAFE_ESCAPES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)%(?:0[0-9a-f]|1[0-9a-f]|7f)").expect("static regex"));
#[allow(clippy::expect_used)]
static ALLOWED_SCHEME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(?:https?://|mailto:)").expect("static regex"));
#[allow(clippy::expect_used)]
static TEXT_URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)(?:https?://|mailto:)[^\s<>"']+"#).expect("static regex"));
#[allow(clippy::expect_used)]
static ANGLE_GROUP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<([^<>]*)>").expect("static regex"));
#[allow(clippy::expect_used)]
static FOLDING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\r?\n[ \t]+").expect("static regex"));
#[allow(clippy::expect_used)]
static ANCHORS: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("a[href]").expect("static selector"));

/// One raw header as mailparser's `headerLines` reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeaderLine {
    /// Lowercase header name.
    pub key: String,
    /// The full raw line (`Name: value`), folding included.
    pub line: String,
}

/// The parts of a parsed message the extractor reads.
#[derive(Clone, Copy, Debug)]
pub struct ParsedMailView<'a> {
    /// The HTML body when the message has an HTML part.
    pub html: Option<&'a str>,
    /// The text body (only consulted when there is no HTML part).
    pub text: Option<&'a str>,
    pub header_lines: &'a [HeaderLine],
}

/// `safeUrl`: keeps usable URL text unchanged, or rejects it.
fn safe_url(raw: &str) -> Option<String> {
    if utf16_len(raw) > URL_LIMIT || UNSAFE_CHARS.is_match(raw) {
        return None;
    }
    if UNSAFE_ESCAPES.is_match(raw) || !ALLOWED_SCHEME.is_match(raw) {
        return None;
    }
    let url = url::Url::parse(raw).ok()?;
    if !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    if url.scheme() == "mailto" {
        return (!url.path().is_empty()).then(|| raw.to_owned());
    }
    url.host_str()
        .filter(|host| !host.is_empty())
        .map(|_| raw.to_owned())
}

/// `[\u0000-\u001f\u007f]` to spaces, `\s+` collapsed, trimmed.
fn clean_label(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    let mut in_space = false;
    for c in label.chars() {
        let c = if c <= '\u{1f}' || c == '\u{7f}' {
            ' '
        } else {
            c
        };
        if is_js_space(c) {
            if !in_space {
                out.push(' ');
            }
            in_space = true;
        } else {
            out.push(c);
            in_space = false;
        }
    }
    js_trim(&out).to_owned()
}

struct LinkCollector {
    links: Vec<EmailLink>,
    seen: HashMap<String, usize>,
    truncated: bool,
}

impl LinkCollector {
    fn add(&mut self, raw: &str, label: &str, source: EmailLinkSource) {
        if utf16_len(raw) > URL_LIMIT {
            self.truncated = true;
        }
        let Some(url) = safe_url(raw) else {
            return;
        };
        let label = clean_label(label);
        if utf16_len(&label) > MAX_LABEL {
            self.truncated = true;
        }
        let bounded = utf16_slice(&label, 0, MAX_LABEL).into_owned();
        if let Some(&index) = self.seen.get(&url) {
            if let Some(existing) = self.links.get_mut(index)
                && PREFERRED_LABEL.is_match(&label)
                && !PREFERRED_LABEL.is_match(&existing.label)
            {
                existing.label = bounded;
            }
            return;
        }
        self.seen.insert(url.clone(), self.links.len());
        self.links.push(EmailLink {
            url,
            label: bounded,
            source,
        });
    }
}

/// `extractEmailLinkMetadata`.
pub fn extract_email_link_metadata(parsed: ParsedMailView<'_>) -> EmailLinkMetadata {
    let mut collector = LinkCollector {
        links: Vec::new(),
        seen: HashMap::new(),
        truncated: false,
    };
    if let Some(full_html) = parsed.html {
        let mut html = utf16_slice(full_html, 0, SCAN_LIMIT).into_owned();
        if utf16_len(full_html) > SCAN_LIMIT {
            collector.truncated = true;
            // Do not let HTML parser recovery invent a partial href at the bound.
            let last_open = html.rfind('<');
            let last_close = html.rfind('>');
            if let Some(open) = last_open
                && last_close.is_none_or(|close| open > close)
            {
                html.truncate(open);
            }
        }
        let document = Html::parse_document(&html);
        for anchor in document.select(&ANCHORS) {
            let href = anchor.value().attr("href").unwrap_or_default();
            let text: String = anchor.text().collect();
            collector.add(href, &text, EmailLinkSource::Html);
        }
    }
    // Text is a source only for messages without an HTML part (mailparser
    // synthesizes text from HTML, including image URLs).
    let plain_text = if parsed.html.is_some() {
        ""
    } else {
        parsed.text.unwrap_or_default()
    };
    let text = utf16_slice(plain_text, 0, SCAN_LIMIT);
    let text_truncated = utf16_len(plain_text) > SCAN_LIMIT;
    collector.truncated |= text_truncated;
    for found in TEXT_URL.find_iter(&text) {
        if text_truncated && found.end() == text.len() {
            continue;
        }
        collector.add(found.as_str(), "", EmailLinkSource::Text);
    }
    // Stable partition: subscription links first so footer links survive.
    collector
        .links
        .sort_by_key(|link| !PREFERRED_LABEL.is_match(&link.label));
    let links_truncated = collector.truncated || collector.links.len() > MAX_LINKS;
    collector.links.truncate(MAX_LINKS);

    EmailLinkMetadata {
        links: collector.links,
        links_truncated,
        list_unsubscribe: list_unsubscribe(parsed.header_lines),
    }
}

fn list_unsubscribe(header_lines: &[HeaderLine]) -> ListUnsubscribe {
    let mut urls: Vec<String> = Vec::new();
    let mut present = false;
    let mut truncated = header_lines.len() > HEADER_COUNT_LIMIT;
    let mut scanned = 0usize;
    let mut post_values: Vec<String> = Vec::new();
    for header in header_lines.iter().take(HEADER_COUNT_LIMIT) {
        // Relevant names are short; avoid scanning attacker-sized keys.
        if utf16_len(&header.key) > 32 {
            continue;
        }
        let key = header.key.to_lowercase();
        if key != "list-unsubscribe" && key != "list-unsubscribe-post" {
            continue;
        }
        if key == "list-unsubscribe" {
            present = true;
        }
        scanned += utf16_len(&header.line);
        if scanned > HEADER_LIMIT {
            truncated = true;
            continue;
        }
        let unfolded = FOLDING.replace_all(&header.line, " ");
        let value = match unfolded.find(':') {
            Some(colon) => &unfolded[colon + 1..],
            None => &unfolded[..],
        };
        let value = js_trim(value);
        if key == "list-unsubscribe-post" {
            post_values.push(value.to_owned());
            continue;
        }
        for group in ANGLE_GROUP.captures_iter(value) {
            let raw = js_trim(group.get(1).map_or("", |m| m.as_str()));
            if utf16_len(raw) > URL_LIMIT {
                truncated = true;
            }
            let Some(url) = safe_url(raw) else {
                continue;
            };
            if urls.contains(&url) {
                continue;
            }
            if urls.len() == MAX_UNSUBSCRIBE_URLS {
                truncated = true;
            } else {
                urls.push(url);
            }
        }
    }
    let post = (!truncated && post_values.len() == 1 && post_values[0] == ONE_CLICK)
        .then(|| ONE_CLICK.to_owned());
    ListUnsubscribe {
        urls,
        post,
        present,
        truncated,
    }
}
