//! X / Twitter posts, threads and X Articles through the FxTwitter API
//! (`src/press-pods/retrievers/x.ts`).

use std::collections::HashSet;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use futures::future::BoxFuture;
use omni_http::{Method, RedirectRule, Url};
use regex::Regex;
use serde::Deserialize;

use super::{ArticleRetriever, RetrieverContext};
use crate::dates::parse_js_date;
use crate::error::PressPodsError;
use crate::types::Article;

pub const FXTWITTER_THREAD_API: &str = "https://api.fxtwitter.com/2/thread";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const X_RESPONSE_MAX_BYTES: usize = 5 * 1024 * 1024;

const X_URL_HOSTS: &[&str] = &[
    "x.com",
    "www.x.com",
    "mobile.x.com",
    "twitter.com",
    "www.twitter.com",
    "mobile.twitter.com",
];

static NAMED_STATUS: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"^/([^/]+)/status/(\d+)(?:/.*)?$").ok());
static WEB_STATUS: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"^/i/web/status/(\d+)(?:/.*)?$").ok());
static MEDIA_CDN: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"(?i)https?://(?:pbs\.twimg\.com|video\.twimg\.com)/\S+").ok());

/// A parsed status permalink.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XStatusUrl {
    pub id: String,
    pub screen_name: String,
    pub canonical_url: String,
}

/// Accepts only a normal X/Twitter status permalink (tracking params allowed).
pub fn parse_x_status_url(value: &str) -> Result<XStatusUrl, String> {
    let url = Url::parse(value).map_err(|_| format!("Invalid X status URL: {value}"))?;
    let hostname = url.host_str().unwrap_or("").to_lowercase();
    if !X_URL_HOSTS.contains(&hostname.as_str()) {
        return Err(format!(
            "Unsupported X URL hostname: {}",
            url.host_str().unwrap_or("")
        ));
    }
    let path = url.path();
    let named = NAMED_STATUS.as_ref().and_then(|re| re.captures(path));
    let web = WEB_STATUS.as_ref().and_then(|re| re.captures(path));
    let (screen_name, id) = match (named, web) {
        (Some(named), _) => (
            named.get(1).map(|m| m.as_str()).unwrap_or("i").to_owned(),
            named
                .get(2)
                .map(|m| m.as_str())
                .unwrap_or_default()
                .to_owned(),
        ),
        (None, Some(web)) => (
            "i".to_owned(),
            web.get(1)
                .map(|m| m.as_str())
                .unwrap_or_default()
                .to_owned(),
        ),
        (None, None) => return Err(format!("X URL is not a status permalink: {value}")),
    };
    Ok(XStatusUrl {
        canonical_url: format!("https://x.com/{screen_name}/status/{id}"),
        id,
        screen_name,
    })
}

/// TS read FxTwitter's status fields structurally (`typeof`, optional
/// chaining), so an unexpected shape in one field never discarded the whole
/// status. Each optional field decodes on its own: a wrong type is `None`.
fn lenient<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).ok())
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct XAuthor {
    #[serde(default, deserialize_with = "lenient")]
    pub id: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub screen_name: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct XMediaItem {
    #[serde(default, deserialize_with = "lenient")]
    pub url: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub alt_text: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    #[serde(rename = "alt_text")]
    pub alt_text_snake: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct XMedia {
    #[serde(default, deserialize_with = "lenient")]
    pub all: Option<Vec<XMediaItem>>,
    #[serde(default, deserialize_with = "lenient")]
    pub photos: Option<Vec<XMediaItem>>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct XArticleBlock {
    #[serde(default, deserialize_with = "lenient")]
    pub text: Option<String>,
    #[serde(rename = "type", default, deserialize_with = "lenient")]
    pub kind: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct XMediaInfo {
    #[serde(default, deserialize_with = "lenient")]
    original_img_url: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct XCoverMedia {
    #[serde(default, deserialize_with = "lenient")]
    media_info: Option<XMediaInfo>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct XArticleContent {
    #[serde(default, deserialize_with = "lenient")]
    blocks: Option<Vec<XArticleBlock>>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct XArticleData {
    #[serde(default, deserialize_with = "lenient")]
    title: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    created_at: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    cover_media: Option<XCoverMedia>,
    #[serde(default, deserialize_with = "lenient")]
    content: Option<XArticleContent>,
}

/// A status as FxTwitter returns it; only complete ones (id, text, author id) count.
#[derive(Clone, Debug, Default, Deserialize)]
struct XStatus {
    #[serde(default, deserialize_with = "lenient")]
    id: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    text: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    created_at: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    created_timestamp: Option<f64>,
    #[serde(default, deserialize_with = "lenient")]
    author: Option<XAuthor>,
    #[serde(default, deserialize_with = "lenient")]
    media: Option<XMedia>,
    #[serde(default, deserialize_with = "lenient")]
    article: Option<XArticleData>,
}

impl XStatus {
    fn author_id(&self) -> Option<&str> {
        self.author.as_ref().and_then(|a| a.id.as_deref())
    }
}

/// `isFullStatus`: decodes a thread entry when it is a complete status.
fn full_status(value: &serde_json::Value) -> Option<XStatus> {
    let object = value.as_object()?;
    let has_strings = object.get("id").is_some_and(serde_json::Value::is_string)
        && object.get("text").is_some_and(serde_json::Value::is_string)
        && object
            .get("author")
            .and_then(|a| a.get("id"))
            .is_some_and(serde_json::Value::is_string);
    if !has_strings {
        return None;
    }
    serde_json::from_value(value.clone()).ok()
}

/// The FxTwitter `/2/thread` envelope.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct FxTwitterThreadResponse {
    pub code: Option<f64>,
    pub status: Option<serde_json::Value>,
    pub thread: Option<Vec<serde_json::Value>>,
    pub author: Option<serde_json::Value>,
    pub message: Option<String>,
    pub error: Option<String>,
}

fn parse_published_at(
    status: &XStatus,
    article: Option<&XArticleData>,
    tz: &jiff::tz::TimeZone,
) -> Option<i64> {
    let value = article
        .and_then(|a| a.created_at.as_deref())
        .or(status.created_at.as_deref());
    if let Some(ms) = value
        .filter(|v| !v.is_empty())
        .and_then(|v| parse_js_date(v, tz))
    {
        return Some(ms);
    }
    #[allow(clippy::cast_possible_truncation)]
    status
        .created_timestamp
        .filter(|t| t.is_finite())
        .map(|t| (t * 1000.0) as i64)
}

fn first_photo_url(status: &XStatus) -> Option<String> {
    status
        .media
        .as_ref()?
        .photos
        .as_ref()?
        .iter()
        .find_map(|photo| photo.url.clone().filter(|u| !u.is_empty()))
}

fn alt_text(item: &XMediaItem) -> Option<&str> {
    item.alt_text.as_deref().or(item.alt_text_snake.as_deref())
}

fn status_media(status: &XStatus) -> Vec<XMediaItem> {
    let Some(media) = status.media.as_ref() else {
        return Vec::new();
    };
    let mut seen = HashSet::new();
    media
        .all
        .iter()
        .flatten()
        .chain(media.photos.iter().flatten())
        .filter(|item| {
            let key = item.url.clone().unwrap_or_else(|| {
                format!(
                    "{}\0{}",
                    item.alt_text.as_deref().unwrap_or(""),
                    item.alt_text_snake.as_deref().unwrap_or("")
                )
            });
            seen.insert(key)
        })
        .cloned()
        .collect()
}

fn status_text(status: &XStatus) -> String {
    let mut text = status.text.as_deref().unwrap_or("").trim().to_owned();
    let media = status_media(status);
    for item in &media {
        if let Some(url) = item.url.as_deref().filter(|u| !u.is_empty()) {
            text = text.replace(url, "");
        }
    }
    if let Some(re) = MEDIA_CDN.as_ref() {
        text = re.replace_all(&text, "").into_owned();
    }
    let text = text
        .split('\n')
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned();
    let descriptions = media
        .iter()
        .filter_map(alt_text)
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(|d| format!("Image description: {d}"));
    std::iter::once(text)
        .chain(descriptions)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// X Article blocks as narration markdown: headings become `##`/`###`, media
/// placeholders become image descriptions, dividers are dropped.
pub fn article_blocks_to_markdown(blocks: &[XArticleBlock]) -> String {
    blocks
        .iter()
        .filter_map(|block| {
            let text = block.text.as_deref()?.trim();
            if text.is_empty() {
                return None;
            }
            match block.kind.as_deref() {
                Some("atomic" | "media") => Some(format!("Image description: {text}")),
                Some("divider") => None,
                Some("header-one" | "header-two") => Some(format!("## {text}")),
                Some("header-three") => Some(format!("### {text}")),
                _ => Some(text.to_owned()),
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The first non-empty line, shortened at a word boundary with `…`.
pub fn thread_title(text: &str, max_length: usize) -> Option<String> {
    let first = text.split('\n').map(str::trim).find(|l| !l.is_empty())?;
    if omni_core::js::utf16_len(first) <= max_length {
        return Some(first.to_owned());
    }
    let available = omni_core::js::utf16_slice(first, 0, max_length.saturating_sub(1)).into_owned();
    let last_space = available
        .rfind(' ')
        .map(|i| omni_core::js::utf16_len(&available[..i]));
    #[allow(clippy::cast_precision_loss)]
    let threshold = max_length as f64 * 0.6;
    #[allow(clippy::cast_precision_loss)]
    let cut_at = match last_space {
        Some(space) if space as f64 > threshold => space,
        _ => omni_core::js::utf16_len(&available),
    };
    let head = omni_core::js::utf16_slice(&available, 0, cut_at);
    Some(format!("{}…", head.trim_end()))
}

/// Builds the article from an FxTwitter thread payload.
pub fn parse_fx_twitter_response(
    payload: &FxTwitterThreadResponse,
    requested: &XStatusUrl,
    tz: &jiff::tz::TimeZone,
) -> Result<Article, String> {
    if payload.code != Some(200.0) {
        let detail = payload
            .message
            .as_deref()
            .or(payload.error.as_deref())
            .unwrap_or("unknown API error");
        let code = payload
            .code
            .map(omni_core::js::number_to_string)
            .unwrap_or_else(|| "missing".to_owned());
        return Err(format!("FxTwitter API returned code {code}: {detail}"));
    }
    let candidates: Vec<&serde_json::Value> = payload
        .status
        .iter()
        .chain(payload.thread.iter().flatten())
        .collect();
    let root = candidates
        .iter()
        .filter_map(|v| full_status(v))
        .find(|status| status.id.as_deref() == Some(requested.id.as_str()))
        .ok_or_else(|| format!("FxTwitter response did not contain status {}", requested.id))?;
    let payload_author_name = payload
        .author
        .as_ref()
        .and_then(|a| a.get("name"))
        .and_then(|n| n.as_str())
        .map(str::to_owned);
    let author = root
        .author
        .as_ref()
        .and_then(|a| a.name.clone())
        .or(payload_author_name);
    let screen_name = root
        .author
        .as_ref()
        .and_then(|a| a.screen_name.clone())
        .unwrap_or_else(|| requested.screen_name.clone());
    let canonical_url = format!("https://x.com/{screen_name}/status/{}", requested.id);
    let article = root.article.as_ref();
    let cover = article
        .and_then(|a| a.cover_media.as_ref())
        .and_then(|c| c.media_info.as_ref())
        .and_then(|m| m.original_img_url.clone());

    if let Some(article) = article
        && let (Some(blocks), Some(title)) = (
            article.content.as_ref().and_then(|c| c.blocks.as_ref()),
            article
                .title
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty()),
        )
    {
        let text = article_blocks_to_markdown(blocks);
        if !text.is_empty() {
            return Ok(Article {
                title: Some(title.to_owned()),
                text,
                author,
                domain: Some("x.com".to_owned()),
                url: canonical_url,
                published_at: parse_published_at(&root, Some(article), tz),
                lead_image_url: cover.or_else(|| first_photo_url(&root)),
            });
        }
    }

    let source: Vec<&serde_json::Value> = match payload.thread.as_ref().filter(|t| !t.is_empty()) {
        Some(thread) => thread.iter().collect(),
        None => Vec::new(),
    };
    let mut seen = HashSet::new();
    let mut statuses: Vec<XStatus> = if source.is_empty() {
        seen.insert(root.id.clone().unwrap_or_default());
        vec![root.clone()]
    } else {
        source
            .into_iter()
            .filter_map(full_status)
            .filter(|status| {
                status.author_id() == root.author_id()
                    && seen.insert(status.id.clone().unwrap_or_default())
            })
            .collect()
    };
    if !seen.contains(root.id.as_deref().unwrap_or_default()) {
        statuses.insert(0, root.clone());
    }
    let text = statuses
        .iter()
        .map(status_text)
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    if text.is_empty() {
        return Err(format!("X thread {} has no readable text", requested.id));
    }
    let title = article
        .and_then(|a| a.title.as_deref())
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .or_else(|| thread_title(&status_text(&root), 120));
    Ok(Article {
        title,
        text,
        author,
        domain: Some("x.com".to_owned()),
        url: canonical_url,
        published_at: parse_published_at(&root, article, tz),
        lead_image_url: cover.or_else(|| first_photo_url(&root)),
    })
}

/// `retrieveArticleX`.
pub struct XRetriever(pub Arc<RetrieverContext>);

impl ArticleRetriever for XRetriever {
    fn name(&self) -> &str {
        "x"
    }

    fn retrieve<'a>(
        &'a self,
        url: &'a str,
        user_agent: &'a str,
    ) -> BoxFuture<'a, Result<Article, PressPodsError>> {
        Box::pin(async move {
            let requested = parse_x_status_url(url)
                .map_err(|m| PressPodsError::failed("parse X status URL", m))?;
            let api = Url::parse(&format!("{FXTWITTER_THREAD_API}/{}", requested.id))
                .map_err(|e| PressPodsError::failed("retrieve X article", e.to_string()))?;
            let response = self
                .0
                .http
                .request(Method::GET, api)
                .header("user-agent", user_agent)
                .header("accept", "application/json")
                .redirect(RedirectRule::Error)
                .timeout(REQUEST_TIMEOUT)
                .send_bounded(X_RESPONSE_MAX_BYTES)
                .await
                .map_err(|e| PressPodsError::http("retrieve X article", e))?;
            if !response.status.is_success() {
                return Err(PressPodsError::failed(
                    "retrieve X article",
                    format!(
                        "FxTwitter request failed with HTTP {}",
                        response.status.as_u16()
                    ),
                ));
            }
            let raw: serde_json::Value = serde_json::from_slice(&response.body)
                .map_err(|e| PressPodsError::invalid("parse X article JSON", e.to_string()))?;
            let decoded: FxTwitterThreadResponse = serde_json::from_value(raw)
                .map_err(|e| PressPodsError::invalid("decode X article response", e.to_string()))?;
            parse_fx_twitter_response(&decoded, &requested, &self.0.tz)
                .map_err(|m| PressPodsError::failed("parse X article response", m))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_status_with_unexpected_field_types_still_counts_as_full() {
        let value = serde_json::json!({
            "id": "1",
            "text": "Hello thread",
            "author": { "id": "a", "name": "Ann", "screen_name": 7 },
            "created_timestamp": "not a number",
            "media": { "photos": "nope" },
            "article": null,
        });
        let status = full_status(&value).unwrap();
        assert_eq!(status.text.as_deref(), Some("Hello thread"));
        assert_eq!(status.created_timestamp, None);
        assert!(status.media.unwrap().photos.is_none());
        assert_eq!(status.author.unwrap().screen_name, None);
    }
}
