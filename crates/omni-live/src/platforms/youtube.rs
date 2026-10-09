//! YouTube live status from the channel's `/live` page.

use std::sync::LazyLock;

use omni_http::public::PublicHttpClient;
use regex::Regex;
use serde_json::Value;

use super::{PLATFORM_HTML_MAX_BYTES, fetch_page_html};
use crate::platform::{FetchedLive, FetchedStatus, Platform};

#[allow(clippy::expect_used)]
static PLAYER_ASSIGNMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bytInitialPlayerResponse\s*=\s*").expect("valid regex"));
#[allow(clippy::expect_used)]
static INITIAL_DATA: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bytInitialData\s*=\s*\{").expect("valid regex"));
#[allow(clippy::expect_used)]
static TITLE_META: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)<meta\s+name="title"\s+content="([^"]*)"\s*/?>"#).expect("valid regex")
});
#[allow(clippy::expect_used)]
static VIEW_COUNT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#""viewCount":\{"runs":\[\{"text":"([0-9,]+)"\}"#).expect("valid regex")
});

/// Pure: the status a `/live` page shows.
pub fn extract_youtube_status(html: &str) -> FetchedStatus {
    let has_assignment = PLAYER_ASSIGNMENT.is_match(html);
    let Some(player_response) = initial_player_response(html) else {
        // An offline channel's /live route renders the channel page: it has
        // ytInitialData but assigns no player response.
        if !has_assignment && INITIAL_DATA.is_match(html) {
            return FetchedStatus::Offline;
        }
        return FetchedStatus::unknown("Response missing expected YouTube data structure");
    };

    let is_live = player_response
        .get("microformat")
        .and_then(|m| m.get("playerMicroformatRenderer"))
        .and_then(|r| r.get("liveBroadcastDetails"))
        .and_then(|d| d.get("isLiveNow"))
        == Some(&Value::Bool(true));
    if !is_live {
        return FetchedStatus::Offline;
    }
    let Some(title) = TITLE_META
        .captures(html)
        .and_then(|c| c.get(1))
        .map(|m| html_escape::decode_html_entities(m.as_str()).into_owned())
    else {
        return FetchedStatus::unknown("Live detected but failed to extract title");
    };
    FetchedStatus::Live(FetchedLive {
        title,
        viewer_count: viewer_count(html),
        category: None,
        started_at: None,
    })
}

/// The first `ytInitialPlayerResponse = {...}` assignment whose object
/// parses and matches the player-response shape.
fn initial_player_response(html: &str) -> Option<Value> {
    PLAYER_ASSIGNMENT.find_iter(html).find_map(|m| {
        let start = m.end();
        if html.as_bytes().get(start) != Some(&b'{') {
            return None;
        }
        let json = extract_json_object(html, start)?;
        let parsed: Value = serde_json::from_str(json).ok()?;
        valid_player_response(&parsed).then_some(parsed)
    })
}

/// Each present key must hold the
/// declared shape (objects all the way down, `isLiveNow` a boolean).
fn valid_player_response(value: &Value) -> bool {
    fn optional_object<'a>(value: &'a Value, key: &str) -> Result<Option<&'a Value>, ()> {
        match value.get(key) {
            None => Ok(None),
            Some(inner) if inner.is_object() => Ok(Some(inner)),
            Some(_) => Err(()),
        }
    }
    let check = || -> Result<(), ()> {
        if !value.is_object() {
            return Err(());
        }
        let Some(microformat) = optional_object(value, "microformat")? else {
            return Ok(());
        };
        let Some(renderer) = optional_object(microformat, "playerMicroformatRenderer")? else {
            return Ok(());
        };
        let Some(details) = optional_object(renderer, "liveBroadcastDetails")? else {
            return Ok(());
        };
        match details.get("isLiveNow") {
            None | Some(Value::Bool(_)) => Ok(()),
            Some(_) => Err(()),
        }
    };
    check().is_ok()
}

/// The balanced `{...}` starting at `start`, honoring JSON strings.
fn extract_json_object(html: &str, start: usize) -> Option<&str> {
    let bytes = html.as_bytes();
    let mut depth = 0i64;
    let mut in_string = false;
    let mut escaped = false;
    for (index, &byte) in bytes.iter().enumerate().skip(start) {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return html.get(start..=index);
                }
            }
            _ => {}
        }
    }
    None
}

fn viewer_count(html: &str) -> Option<i64> {
    let digits: String = VIEW_COUNT
        .captures(html)?
        .get(1)?
        .as_str()
        .chars()
        .filter(|c| *c != ',')
        .collect();
    digits.parse().ok()
}

#[derive(Clone)]
pub struct YouTubeClient {
    http: PublicHttpClient,
}

impl YouTubeClient {
    pub fn new(http: PublicHttpClient) -> Self {
        Self { http }
    }

    pub async fn fetch_live_status(&self, username: &str) -> FetchedStatus {
        let page = Platform::YouTube.live_url(username);
        match fetch_page_html(&self.http, &page, PLATFORM_HTML_MAX_BYTES).await {
            Ok(html) => extract_youtube_status(&html),
            Err(error) => FetchedStatus::unknown(error.message),
        }
    }
}
