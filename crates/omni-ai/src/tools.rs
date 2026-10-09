//! Built-in AI tools (`src/ai/tools/*`): Tavily web search and public page fetching.

use std::time::Duration;

use futures::future::BoxFuture;
use omni_api::costs::{CostCategory, CostPriceStatus, CostUsage};
use omni_http::public::PublicHttpClient;
use omni_http::{HttpClient, HttpError, Method, RedirectRule, RequestBuilder, Url};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::costs::{CostRecorder, NewCostEvent, current_cost_feature};
use crate::{AiTool, ToolSpec};

pub const TAVILY_SEARCH_URL: &str = "https://api.tavily.com/search";
pub const TAVILY_RESPONSE_MAX_BYTES: usize = 1024 * 1024;
pub const TAVILY_TIMEOUT: Duration = Duration::from_secs(15);
/// `PUBLIC_TEXT_MAX_BYTES`.
pub const PUBLIC_TEXT_MAX_BYTES: usize = 10 * 1024 * 1024;
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(15);
/// `MAX_OUTPUT_CHARS` (UTF-16 units).
pub const MAX_OUTPUT_CHARS: usize = 20_000;
/// got's default redirect limit, which `publicGotStream` keeps.
const MAX_REDIRECTS: u8 = 10;

/// Either the SSRF-guarded public client (production) or a plain client (tests that
/// point the endpoint at a local mock server).
#[derive(Clone, Debug)]
enum Transport {
    Public(PublicHttpClient),
    Plain(HttpClient),
}

impl Transport {
    fn request(&self, method: Method, url: Url) -> RequestBuilder {
        match self {
            Transport::Public(client) => client.request(method, url),
            Transport::Plain(client) => client.request(method, url),
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum WebSearchError {
    #[error("Web search failed: {0}")]
    Http(#[from] HttpError),
    #[error("Web search failed: invalid Tavily response: {0}")]
    Decode(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchTopic {
    General,
    News,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TimeRange {
    Day,
    Week,
    Month,
    Year,
}

/// `searchWebEffect` options.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SearchOptions {
    pub query: String,
    pub topic: Option<SearchTopic>,
    pub time_range: Option<TimeRange>,
    /// Default 5.
    pub max_results: Option<u32>,
    /// Truncates each result's content to this many UTF-16 units.
    pub max_content_chars: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WebSearchResult {
    pub title: String,
    pub url: String,
    pub content: String,
}

/// `{ results, responseTime }`, the tool output the model sees.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebSearchResults {
    pub results: Vec<WebSearchResult>,
    pub response_time: f64,
}

#[derive(Deserialize)]
struct TavilyResponse {
    results: Vec<WebSearchResult>,
    response_time: f64,
}

#[derive(Serialize)]
struct TavilyRequest<'a> {
    query: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    topic: Option<SearchTopic>,
    #[serde(skip_serializing_if = "Option::is_none")]
    time_range: Option<TimeRange>,
    max_results: u32,
}

/// Tavily web search (15 s timeout, 1 MiB response cap, 0.8 cents per call).
#[derive(Clone)]
pub struct WebSearch {
    transport: Transport,
    api_key: String,
    costs: CostRecorder,
    max_response_bytes: usize,
}

impl WebSearch {
    /// Production: requests go through the SSRF-guarded public client.
    pub fn new(http: PublicHttpClient, api_key: String, costs: CostRecorder) -> Self {
        Self {
            transport: Transport::Public(http),
            api_key,
            costs,
            max_response_bytes: TAVILY_RESPONSE_MAX_BYTES,
        }
    }

    /// A plain client (tests rewrite the Tavily origin to a local mock server, which the
    /// public client would refuse).
    pub fn with_http(http: HttpClient, api_key: String, costs: CostRecorder) -> Self {
        Self {
            transport: Transport::Plain(http),
            api_key,
            costs,
            max_response_bytes: TAVILY_RESPONSE_MAX_BYTES,
        }
    }

    /// Overrides the response byte cap (tests).
    pub fn with_max_response_bytes(self, max_response_bytes: usize) -> Self {
        Self {
            max_response_bytes,
            ..self
        }
    }

    /// `searchWebEffect`: one Tavily basic search, recorded as a `search` cost event.
    pub async fn search(&self, options: SearchOptions) -> Result<WebSearchResults, WebSearchError> {
        let url =
            Url::parse(TAVILY_SEARCH_URL).map_err(|e| HttpError::InvalidUrl(e.to_string()))?;
        let response = self
            .transport
            .request(Method::POST, url)
            .bearer_auth(&self.api_key)
            .json(&TavilyRequest {
                query: &options.query,
                topic: options.topic,
                time_range: options.time_range,
                max_results: options.max_results.unwrap_or(5),
            })
            .timeout(TAVILY_TIMEOUT)
            .redirect(RedirectRule::Follow(MAX_REDIRECTS))
            .send_bounded(self.max_response_bytes)
            .await?;
        if !response.status.is_success() {
            return Err(WebSearchError::Http(HttpError::Status {
                status: response.status.as_u16(),
                body: String::from_utf8_lossy(&response.body).into_owned(),
            }));
        }
        let decoded: TavilyResponse = serde_json::from_slice(&response.body)
            .map_err(|e| WebSearchError::Decode(e.to_string()))?;

        // A basic search consumes one credit; Tavily's pay-as-you-go rate is the estimate.
        self.costs
            .record(NewCostEvent {
                category: CostCategory::Search,
                feature: current_cost_feature("web-search").to_owned(),
                operation: "search".to_owned(),
                service: "tavily".to_owned(),
                model: Some("basic".to_owned()),
                cost_cents: Some(0.8),
                price_status: CostPriceStatus::Estimated,
                usage: CostUsage {
                    requests: Some(1.0),
                    credits: Some(1.0),
                    ..CostUsage::default()
                },
                event_id: None,
                incurred_at: None,
                run_id: None,
            })
            .await;

        Ok(WebSearchResults {
            results: decoded
                .results
                .into_iter()
                .map(|result| WebSearchResult {
                    content: match options.max_content_chars {
                        Some(max) => {
                            omni_core::js::utf16_slice(&result.content, 0, max).into_owned()
                        }
                        None => result.content,
                    },
                    ..result
                })
                .collect(),
            response_time: decoded.response_time,
        })
    }
}

#[derive(Deserialize)]
struct WebSearchArgs {
    query: String,
    #[serde(default)]
    topic: Option<SearchTopic>,
    #[serde(default)]
    time_range: Option<TimeRange>,
}

impl AiTool for WebSearch {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "web_search".to_owned(),
            description: "Search the web for current information. Use topic 'news' for current events and breaking news.".to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "The search query" },
                    "topic": {
                        "type": "string",
                        "enum": ["general", "news"],
                        "description": "'general' for broad searches, 'news' for current events"
                    },
                    "time_range": {
                        "type": "string",
                        "enum": ["day", "week", "month", "year"],
                        "description": "Filter results by recency"
                    }
                },
                "required": ["query"],
                "additionalProperties": false
            }),
        }
    }

    fn call<'a>(&'a self, args: Value) -> BoxFuture<'a, Result<Value, String>> {
        Box::pin(async move {
            let args: WebSearchArgs =
                serde_json::from_value(args).map_err(|e| format!("Invalid input: {e}"))?;
            let results = self
                .search(SearchOptions {
                    query: args.query,
                    topic: args.topic,
                    time_range: args.time_range,
                    ..SearchOptions::default()
                })
                .await
                .map_err(|e| e.to_string())?;
            serde_json::to_value(results).map_err(|e| e.to_string())
        })
    }
}

#[derive(thiserror::Error, Debug)]
#[error("Fetch {url} failed: {source}")]
pub struct FetchUrlError {
    pub url: String,
    #[source]
    pub source: HttpError,
}

/// `HtmlToMarkdownResult`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HtmlToMarkdown {
    pub title: Option<String>,
    pub content: String,
    pub truncated: bool,
}

/// Fetches a public page through the SSRF-guarded client and converts it to markdown.
#[derive(Clone)]
pub struct FetchUrl {
    transport: Transport,
}

impl FetchUrl {
    pub fn new(http: PublicHttpClient) -> Self {
        Self {
            transport: Transport::Public(http),
        }
    }

    /// A plain client (tests against a local mock server).
    pub fn with_http(http: HttpClient) -> Self {
        Self {
            transport: Transport::Plain(http),
        }
    }

    /// `fetchUrlEffect`: GET (redirects followed and revalidated), 10 MiB cap, non-2xx is
    /// an error, then [`html_to_markdown`].
    pub async fn fetch(&self, url: &str) -> Result<HtmlToMarkdown, FetchUrlError> {
        let fail = |source| FetchUrlError {
            url: url.to_owned(),
            source,
        };
        let parsed = match &self.transport {
            Transport::Public(_) => omni_http::public::assert_public_http_url_syntax(url),
            Transport::Plain(_) => {
                Url::parse(url).map_err(|e| HttpError::InvalidUrl(e.to_string()))
            }
        }
        .map_err(fail)?;
        let response = self
            .transport
            .request(Method::GET, parsed)
            .header("Accept", "text/html")
            .timeout(FETCH_TIMEOUT)
            .redirect(RedirectRule::Follow(MAX_REDIRECTS))
            .send_bounded(PUBLIC_TEXT_MAX_BYTES)
            .await
            .map_err(fail)?;
        if !response.status.is_success() {
            return Err(fail(HttpError::Status {
                status: response.status.as_u16(),
                body: String::new(),
            }));
        }
        Ok(html_to_markdown(&String::from_utf8_lossy(&response.body)))
    }
}

#[derive(Deserialize)]
struct FetchUrlArgs {
    url: String,
}

impl AiTool for FetchUrl {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "fetch_url".to_owned(),
            description: "Fetch a web page and return its content as clean markdown. Use this to read full articles, documentation, or other pages found via web_search.".to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "format": "uri", "description": "The URL to fetch" }
                },
                "required": ["url"],
                "additionalProperties": false
            }),
        }
    }

    fn call<'a>(&'a self, args: Value) -> BoxFuture<'a, Result<Value, String>> {
        Box::pin(async move {
            let args: FetchUrlArgs =
                serde_json::from_value(args).map_err(|e| format!("Invalid input: {e}"))?;
            if Url::parse(&args.url).is_err() {
                return Err("Invalid input: url must be a valid URL".to_owned());
            }
            let page = self.fetch(&args.url).await.map_err(|e| e.to_string())?;
            serde_json::to_value(page).map_err(|e| e.to_string())
        })
    }
}

/// `htmlToMarkdown`: Readability when the page looks readerable, otherwise the
/// `<main>`/`<article>`/`<body>` of the page with chrome removed; markdown via turndown
/// rules (ATX headings, fenced code); a `# title` heading; truncated to
/// [`MAX_OUTPUT_CHARS`] UTF-16 units.
pub fn html_to_markdown(html: &str) -> HtmlToMarkdown {
    let mut title = None;
    let mut content_html = None;
    if let Ok(mut readability) = dom_smoothie::Readability::new(html, None, None)
        && readability.is_probably_readable()
    {
        match readability.parse() {
            Ok(article) if !article.content.is_empty() => {
                content_html = Some(article.content.to_string());
                title = Some(article.title).filter(|t| !t.is_empty());
            }
            _ => {}
        }
    }
    let content_html = match content_html {
        Some(content) => content,
        None => {
            let (extracted, page_title) = fallback_extract(html);
            title = page_title;
            extracted
        }
    };
    let converter = htmd::HtmlToMarkdown::builder()
        .options(htmd::options::Options {
            heading_style: htmd::options::HeadingStyle::Atx,
            code_block_style: htmd::options::CodeBlockStyle::Fenced,
            ..htmd::options::Options::default()
        })
        .skip_tags(vec!["script", "style"])
        .build();
    let mut markdown = converter.convert(&content_html).unwrap_or_default();
    if let Some(title) = &title {
        markdown = format!("# {title}\n\n{markdown}");
    }
    let truncated = omni_core::js::utf16_len(&markdown) > MAX_OUTPUT_CHARS;
    if truncated {
        markdown = omni_core::js::utf16_slice(&markdown, 0, MAX_OUTPUT_CHARS).into_owned();
    }
    HtmlToMarkdown {
        title,
        content: markdown,
        truncated,
    }
}

/// `fallbackExtract` plus the `<title>` text: drops script/style/nav/footer/header/
/// aside/svg, then prefers `main`, `article`, `[role=main]`, else the body.
fn fallback_extract(html: &str) -> (String, Option<String>) {
    let document = scraper::Html::parse_document(html);
    let title = scraper::Selector::parse("title")
        .ok()
        .and_then(|selector| document.select(&selector).next())
        .map(|el| el.text().collect::<String>().trim().to_owned());
    let removed: Vec<_> = ["script", "style", "nav", "footer", "header", "aside", "svg"]
        .iter()
        .filter_map(|tag| scraper::Selector::parse(tag).ok())
        .flat_map(|selector| {
            document
                .select(&selector)
                .map(|el| el.id())
                .collect::<Vec<_>>()
        })
        .collect();
    let mut document = document;
    for id in removed {
        if let Some(mut node) = document.tree.get_mut(id) {
            node.detach();
        }
    }
    let pick = |selector: &str| {
        scraper::Selector::parse(selector)
            .ok()
            .and_then(|s| document.select(&s).next().map(|el| el.inner_html()))
    };
    let content = pick("main, article, [role='main']")
        .or_else(|| pick("body"))
        .unwrap_or_else(|| html.to_owned());
    (content, title)
}
