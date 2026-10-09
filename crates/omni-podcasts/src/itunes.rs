//! Keyless iTunes Search API for podcast shows.

use std::time::Duration;

use omni_http::public::PublicHttpClient;
use omni_http::{Method, Url};
use serde::Deserialize;

use crate::titles::normalize_title;

pub const SEARCH_URL: &str = "https://itunes.apple.com/search";
pub const DEFAULT_LIMIT: u32 = 5;
pub const ITUNES_RESPONSE_MAX_BYTES: usize = 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// A podcast show as returned by the iTunes Search API.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ItunesShow {
    pub itunes_id: i64,
    pub title: String,
    pub feed_url: Option<String>,
    pub artwork_url: Option<String>,
    pub genres: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ItunesResult {
    collection_id: Option<f64>,
    collection_name: Option<String>,
    feed_url: Option<String>,
    artwork_url600: Option<String>,
    artwork_url100: Option<String>,
    genres: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct ItunesSearchResponse {
    #[serde(default)]
    results: Vec<ItunesResult>,
}

/// `ItunesRequestError`.
#[derive(Debug, thiserror::Error)]
#[error("iTunes search failed for {term}: {detail}")]
pub struct ItunesRequestError {
    pub term: String,
    pub detail: String,
}

/// Decodes a search response; results without an id or name are skipped,
/// structurally invalid bodies fail.
pub fn parse_itunes_response(
    term: &str,
    text: &str,
) -> Result<Vec<ItunesShow>, ItunesRequestError> {
    let parsed: ItunesSearchResponse =
        serde_json::from_str(text).map_err(|e| ItunesRequestError {
            term: term.to_owned(),
            detail: e.to_string(),
        })?;
    Ok(parsed
        .results
        .into_iter()
        .filter_map(|r| {
            let id = r.collection_id.filter(|id| *id != 0.0 && !id.is_nan())?;
            let title = r.collection_name.filter(|n| !n.is_empty())?;
            Some(ItunesShow {
                itunes_id: id as i64,
                title,
                feed_url: r.feed_url,
                artwork_url: r.artwork_url600.or(r.artwork_url100),
                genres: r.genres.unwrap_or_default(),
            })
        })
        .collect())
}

pub async fn search_itunes_podcasts(
    http: &PublicHttpClient,
    term: &str,
    limit: u32,
    max_response_bytes: usize,
) -> Result<Vec<ItunesShow>, ItunesRequestError> {
    let fail = |detail: String| ItunesRequestError {
        term: term.to_owned(),
        detail,
    };
    let url = Url::parse(SEARCH_URL).map_err(|e| fail(e.to_string()))?;
    let limit = limit.to_string();
    let response = http
        .request(Method::GET, url)
        .query(&[
            ("media", "podcast"),
            ("entity", "podcast"),
            ("term", term),
            ("limit", &limit),
        ])
        .header("User-Agent", omni_http::USER_AGENT)
        .timeout(REQUEST_TIMEOUT)
        .send_bounded(max_response_bytes)
        .await
        .map_err(|e| fail(e.to_string()))?;
    if !response.status.is_success() {
        return Err(fail(format!("HTTP {}", response.status.as_u16())));
    }
    parse_itunes_response(term, &String::from_utf8_lossy(&response.body))
}

/// Exact normalized title match, else containment either way, else `None`.
pub fn pick_best_show_match<'a>(
    shows: &'a [ItunesShow],
    show_title: &str,
) -> Option<&'a ItunesShow> {
    let target = normalize_title(show_title);
    if target.is_empty() {
        return None;
    }
    shows
        .iter()
        .find(|show| normalize_title(&show.title) == target)
        .or_else(|| {
            shows.iter().find(|show| {
                let normalized = normalize_title(&show.title);
                !normalized.is_empty()
                    && (normalized.contains(&target) || target.contains(&normalized))
            })
        })
}
