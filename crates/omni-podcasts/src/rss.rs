//! Podcast RSS reading.
//!
//! The TS version used linkedom's forgiving XML DOM; this one walks quick-xml
//! events leniently (stray `&`/`<` escaped first as literal text, mismatched
//! end tags tolerated, parsing stops at the first remaining hard error and
//! keeps what it read) and reproduces the DOM semantics
//! the parser relied on: `getElementsByTagName` over each `<item>`'s
//! descendants, `textContent` concatenation, XML entity decoding followed by
//! the HTML entity pass.

use std::sync::LazyLock;
use std::time::Duration;

use jiff::tz::TimeZone;
use omni_http::public::PublicHttpClient;
use omni_http::{Method, Url};
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use regex::Regex;

use crate::js::parse_date;
use crate::titles::normalize_title;

const DEFAULT_MAX_EPISODES: usize = 30;
const DESCRIPTION_MAX_CHARS: usize = 500;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// `PUBLIC_TEXT_MAX_BYTES`.
pub const FEED_MAX_BYTES: usize = 10 * 1024 * 1024;

/// One episode parsed from a feed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FeedEpisode {
    pub guid: String,
    pub title: String,
    pub published_at: i64,
    pub duration_minutes: Option<i64>,
    pub description: String,
    pub link: Option<String>,
    /// Enclosure URL: the reliable cross-system episode key.
    pub enclosure_url: Option<String>,
}

/// `FeedRequestError`.
#[derive(Debug, thiserror::Error)]
#[error("fetch public podcast feed failed for {feed_url}: {detail}")]
pub struct FeedRequestError {
    pub feed_url: String,
    pub detail: String,
}

/// `fetchFeedEpisodesEffect`.
pub async fn fetch_feed_episodes(
    http: &PublicHttpClient,
    feed_url: &str,
    max_episodes: usize,
    tz: &TimeZone,
) -> Result<Vec<FeedEpisode>, FeedRequestError> {
    let fail = |detail: String| FeedRequestError {
        feed_url: feed_url.to_owned(),
        detail,
    };
    let url = Url::parse(feed_url).map_err(|e| fail(e.to_string()))?;
    let response = http
        .request(Method::GET, url)
        .header("User-Agent", omni_http::USER_AGENT)
        .timeout(REQUEST_TIMEOUT)
        .send_bounded(FEED_MAX_BYTES)
        .await
        .map_err(|e| fail(e.to_string()))?;
    if !response.status.is_success() {
        return Err(fail(format!("HTTP {}", response.status.as_u16())));
    }
    Ok(parse_feed_episodes(
        &String::from_utf8_lossy(&response.body),
        max_episodes,
        tz,
    ))
}

#[derive(Debug, Default)]
struct Element {
    name: String,
    text: String,
    url_attribute: Option<String>,
}

#[derive(Debug, Default)]
struct Item {
    /// Indices into the element arena, in document order.
    descendants: Vec<usize>,
}

impl Item {
    fn first<'a>(&self, arena: &'a [Element], name: &str) -> Option<&'a Element> {
        self.descendants
            .iter()
            .map(|i| &arena[*i])
            .find(|e| e.name == name)
    }

    fn first_text<'a>(&self, arena: &'a [Element], name: &str) -> Option<&'a str> {
        self.first(arena, name).map(|e| e.text.as_str())
    }
}

/// XML-decodes `&name;` / `&#n;` references; unknown names stay literal (as an
/// XML DOM leaves them for the later HTML entity pass).
fn resolve_general_ref(name: &str) -> String {
    if let Some(number) = name.strip_prefix('#') {
        let code = match number.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok(),
            None => number.parse::<u32>().ok(),
        };
        if let Some(c) = code.and_then(char::from_u32) {
            return c.to_string();
        }
        return format!("&{name};");
    }
    match name {
        "amp" => "&".to_owned(),
        "lt" => "<".to_owned(),
        "gt" => ">".to_owned(),
        "quot" => "\"".to_owned(),
        "apos" => "'".to_owned(),
        other => format!("&{other};"),
    }
}

static ATTRIBUTE_REF: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"&(#?[A-Za-z0-9]+);").ok());

fn unescape_attribute(raw: &str) -> String {
    match ATTRIBUTE_REF.as_ref() {
        Some(re) => re
            .replace_all(raw, |caps: &regex::Captures<'_>| {
                resolve_general_ref(&caps[1])
            })
            .into_owned(),
        None => raw.to_owned(),
    }
}

fn url_attribute(start: &BytesStart<'_>) -> Option<String> {
    start
        .attributes()
        .with_checks(false)
        .flatten()
        .find_map(|attribute| {
            (AsRef::<str>::as_ref(&attribute.key) == "url")
                .then(|| unescape_attribute(&attribute.value))
        })
}

type OpenElement = (String, Option<usize>, Option<usize>);

fn push_text(arena: &mut [Element], open: &[OpenElement], text: &str) {
    for (_, index, _) in open {
        if let Some(i) = index {
            arena[*i].text.push_str(text);
        }
    }
}

/// Whether `rest` (just after a `&`) starts a well-formed reference body
/// (`name;`, `#digits;` or `#xhex;`).
fn starts_reference(rest: &str) -> bool {
    let Some(end) = rest.find(';') else {
        return false;
    };
    let body = &rest[..end];
    if let Some(number) = body.strip_prefix('#') {
        return match number.strip_prefix(['x', 'X']) {
            Some(hex) => !hex.is_empty() && hex.chars().all(|c| c.is_ascii_hexdigit()),
            None => !number.is_empty() && number.chars().all(|c| c.is_ascii_digit()),
        };
    }
    body.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && body.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Escapes the stray `&` and `<` characters a forgiving DOM parser (linkedom's
/// htmlparser2 in XML mode) reads as literal text, so one `AT&T` or `a < b`
/// in a feed does not end the walk and lose every later item. CDATA sections
/// and comments are copied verbatim.
pub fn escape_stray_markup(xml: &str) -> std::borrow::Cow<'_, str> {
    if !xml.contains('&') && !xml.contains('<') {
        return std::borrow::Cow::Borrowed(xml);
    }
    let mut out = String::with_capacity(xml.len());
    let mut rest = xml;
    while let Some(index) = rest.find(['&', '<']) {
        out.push_str(&rest[..index]);
        let tail = &rest[index..];
        let verbatim = [("<![CDATA[", "]]>"), ("<!--", "-->")]
            .into_iter()
            .find(|(open, _)| tail.starts_with(open));
        if let Some((open, close)) = verbatim {
            let end = tail[open.len()..]
                .find(close)
                .map_or(tail.len(), |i| open.len() + i + close.len());
            out.push_str(&tail[..end]);
            rest = &tail[end..];
            continue;
        }
        // `find` stopped on a one-byte ASCII `&` or `<`.
        let after = &tail[1..];
        if tail.starts_with('&') {
            if starts_reference(after) {
                out.push('&');
            } else {
                out.push_str("&amp;");
            }
        } else {
            let opens_markup = after
                .chars()
                .next()
                .is_some_and(|c| c.is_alphabetic() || matches!(c, '/' | '!' | '?' | '_' | ':'));
            out.push_str(if opens_markup { "<" } else { "&lt;" });
        }
        rest = after;
    }
    out.push_str(rest);
    std::borrow::Cow::Owned(out)
}

/// Lenient pass over the document collecting every `<item>` and its descendants.
fn collect_items(xml: &str) -> (Vec<Element>, Vec<Item>) {
    let xml = escape_stray_markup(xml);
    let mut reader = Reader::from_str(&xml);
    let config = reader.config_mut();
    config.check_end_names = false;
    config.allow_unmatched_ends = true;
    config.trim_text(false);

    let mut arena: Vec<Element> = Vec::new();
    let mut items: Vec<Item> = Vec::new();
    // Open elements: (name, arena index when recorded, item index when it is an item).
    let mut open: Vec<OpenElement> = Vec::new();
    let mut open_items: Vec<usize> = Vec::new();

    // A hard XML error ends the walk; what was read so far is kept.
    while let Ok(event) = reader.read_event() {
        match event {
            Event::Start(start) => {
                let name = AsRef::<str>::as_ref(&start.name()).to_owned();
                let recorded = (!open_items.is_empty()).then(|| {
                    arena.push(Element {
                        name: name.clone(),
                        text: String::new(),
                        url_attribute: url_attribute(&start),
                    });
                    let index = arena.len() - 1;
                    for item in &open_items {
                        items[*item].descendants.push(index);
                    }
                    index
                });
                let item = (name == "item").then(|| {
                    items.push(Item::default());
                    items.len() - 1
                });
                if let Some(item) = item {
                    open_items.push(item);
                }
                open.push((name, recorded, item));
            }
            Event::Empty(start) => {
                if open_items.is_empty() {
                    continue;
                }
                let name = AsRef::<str>::as_ref(&start.name()).to_owned();
                arena.push(Element {
                    name,
                    text: String::new(),
                    url_attribute: url_attribute(&start),
                });
                let index = arena.len() - 1;
                for item in &open_items {
                    items[*item].descendants.push(index);
                }
            }
            Event::End(end) => {
                let name = AsRef::<str>::as_ref(&end.name()).to_owned();
                if let Some(position) = open.iter().rposition(|(n, _, _)| *n == name) {
                    for (_, _, item) in open.drain(position..) {
                        if item.is_some() {
                            open_items.pop();
                        }
                    }
                }
            }
            Event::Text(text) => {
                let decoded = AsRef::<str>::as_ref(&text).to_owned();
                push_text(&mut arena, &open, &decoded);
            }
            Event::CData(data) => {
                let decoded = AsRef::<str>::as_ref(&data).to_owned();
                push_text(&mut arena, &open, &decoded);
            }
            Event::GeneralRef(reference) => {
                let name = AsRef::<str>::as_ref(&reference).to_owned();
                push_text(&mut arena, &open, &resolve_general_ref(&name));
            }
            Event::Eof => break,
            _ => {}
        }
    }
    (arena, items)
}

static CDATA: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"^\s*<!\[CDATA\[([\s\S]*)\]\]>\s*$").ok());
static TAGS: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"<[^>]*>").ok());
static WHITESPACE: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"\s+").ok());
static DIGITS: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"^\d+$").ok());

fn strip_cdata(text: &str) -> String {
    CDATA
        .as_ref()
        .and_then(|re| re.captures(text))
        .and_then(|caps| caps.get(1))
        .map_or_else(|| text.to_owned(), |m| m.as_str().to_owned())
}

fn replace(re: &LazyLock<Option<Regex>>, text: &str, with: &str) -> String {
    match re.as_ref() {
        Some(re) => re.replace_all(text, with).into_owned(),
        None => text.to_owned(),
    }
}

fn html_decode(text: &str) -> String {
    html_escape::decode_html_entities(text).into_owned()
}

fn is_digits(value: &str) -> bool {
    DIGITS.as_ref().is_some_and(|re| re.is_match(value))
}

fn js_round(x: f64) -> i64 {
    (x + 0.5).floor() as i64
}

/// Plain seconds (`"3720"`), `MM:SS`, and `HH:MM:SS`.
fn parse_duration_minutes(raw: Option<&str>) -> Option<i64> {
    let value = strip_cdata(raw?).trim().to_owned();
    if value.is_empty() {
        return None;
    }
    if is_digits(&value) {
        return Some(js_round(value.parse::<f64>().ok()? / 60.0));
    }
    let parts: Vec<&str> = value.split(':').collect();
    if !(2..=3).contains(&parts.len()) || !parts.iter().all(|p| is_digits(p)) {
        return None;
    }
    let numbers: Vec<f64> = parts
        .iter()
        .map(|p| p.parse::<f64>().unwrap_or(f64::NAN))
        .collect();
    let total = match numbers.as_slice() {
        [h, m, s] => h * 3600.0 + m * 60.0 + s,
        [m, s] => m * 60.0 + s,
        _ => return None,
    };
    Some(js_round(total / 60.0))
}

fn clean_description(raw: &str) -> String {
    let decoded = html_decode(&strip_cdata(raw));
    let text = replace(&WHITESPACE, &replace(&TAGS, &decoded, " "), " ")
        .trim()
        .to_owned();
    if omni_core::js::utf16_len(&text) > DESCRIPTION_MAX_CHARS {
        omni_core::js::utf16_slice(&text, 0, DESCRIPTION_MAX_CHARS).into_owned()
    } else {
        text
    }
}

fn non_empty_trimmed(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
}

/// Parses RSS `<item>`s; items without a usable guid or a parseable pubDate
/// are skipped. Newest first, capped at `max_episodes`.
pub fn parse_feed_episodes(xml: &str, max_episodes: usize, tz: &TimeZone) -> Vec<FeedEpisode> {
    let (arena, items) = collect_items(xml);
    let mut episodes = Vec::new();
    for item in &items {
        let enclosure_url = non_empty_trimmed(
            item.first(&arena, "enclosure")
                .and_then(|e| e.url_attribute.as_deref()),
        );
        let link = non_empty_trimmed(item.first_text(&arena, "link"));
        let Some(guid) = non_empty_trimmed(item.first_text(&arena, "guid"))
            .or_else(|| enclosure_url.clone())
            .or_else(|| link.clone())
        else {
            continue;
        };
        let raw_title = item.first_text(&arena, "title").unwrap_or_default();
        let title = html_decode(&strip_cdata(raw_title)).trim().to_owned();
        let Some(published_at) = item
            .first_text(&arena, "pubDate")
            .filter(|raw| !raw.is_empty())
            .and_then(|raw| parse_date(strip_cdata(raw).trim(), tz))
        else {
            continue;
        };
        let duration_minutes = parse_duration_minutes(item.first_text(&arena, "itunes:duration"));
        let raw_description = item
            .first_text(&arena, "description")
            .or_else(|| item.first_text(&arena, "itunes:summary"))
            .unwrap_or_default();
        episodes.push(FeedEpisode {
            guid,
            title,
            published_at,
            duration_minutes,
            description: clean_description(raw_description),
            link,
            enclosure_url,
        });
    }
    episodes.sort_by_key(|e| std::cmp::Reverse(e.published_at));
    episodes.truncate(max_episodes);
    episodes
}

/// [`parse_feed_episodes`] with the default cap of 30.
pub fn parse_feed_episodes_default(xml: &str, tz: &TimeZone) -> Vec<FeedEpisode> {
    parse_feed_episodes(xml, DEFAULT_MAX_EPISODES, tz)
}

/// Exact normalized title wins, else the longest containment match.
pub fn find_episode_by_title<'a>(
    episodes: &'a [FeedEpisode],
    episode_title: &str,
) -> Option<&'a FeedEpisode> {
    let target = normalize_title(episode_title);
    if target.is_empty() {
        return None;
    }
    if let Some(exact) = episodes
        .iter()
        .find(|e| normalize_title(&e.title) == target)
    {
        return Some(exact);
    }
    let mut best: Option<&FeedEpisode> = None;
    let mut best_length: Option<usize> = None;
    for episode in episodes {
        let normalized = normalize_title(&episode.title);
        if normalized.is_empty() {
            continue;
        }
        if normalized.contains(&target) || target.contains(&normalized) {
            let length = omni_core::js::utf16_len(&normalized);
            if best_length.is_none_or(|b| length > b) {
                best = Some(episode);
                best_length = Some(length);
            }
        }
    }
    best
}
