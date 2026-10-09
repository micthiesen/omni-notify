//! DOM helpers shared by the local extractors: page metadata from `<meta>`,
//! JSON-LD and the `<title>`, plus Mercury-style junk removal.

use scraper::{ElementRef, Html, Selector};

/// `extractTitleFromHtml`: the raw `<title>` text (not entity-decoded), trimmed.
pub fn extract_title_from_html(html: &str) -> Option<String> {
    static TITLE: std::sync::LazyLock<Option<regex::Regex>> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"(?i)<title>([^<]*)</title>").ok());
    let title = TITLE.as_ref()?.captures(html)?.get(1)?.as_str().trim();
    (!title.is_empty()).then(|| title.to_owned())
}

/// `extractDomain`: the URL's hostname.
pub fn extract_domain(url: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
}

pub(crate) fn selector(css: &str) -> Option<Selector> {
    Selector::parse(css).ok()
}

/// The first non-empty `content` of the `<meta>` tags matching `selectors`, in order.
pub fn meta_content(document: &Html, selectors: &[&str]) -> Option<String> {
    selectors.iter().find_map(|css| {
        let sel = selector(css)?;
        document.select(&sel).find_map(|el| {
            el.value()
                .attr("content")
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
        })
    })
}

/// Collapsed text of the first element matching any of `selectors`.
pub fn first_text(document: &Html, selectors: &[&str]) -> Option<String> {
    selectors.iter().find_map(|css| {
        let sel = selector(css)?;
        document
            .select(&sel)
            .map(|el| collapse(&el.text().collect::<String>()))
            .find(|t| !t.is_empty())
    })
}

/// Collapses whitespace runs to single spaces and trims.
pub fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Article metadata the extractors look for beyond the body.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageMeta {
    pub title: Option<String>,
    pub author: Option<String>,
    pub published: Option<String>,
    pub image: Option<String>,
    /// `articleBody` of a JSON-LD article object.
    pub json_ld_body: Option<String>,
}

const ARTICLE_TYPES: &[&str] = &[
    "Article",
    "NewsArticle",
    "BlogPosting",
    "ReportageNewsArticle",
    "AnalysisNewsArticle",
    "OpinionNewsArticle",
    "Report",
    "ScholarlyArticle",
    "TechArticle",
    "SocialMediaPosting",
    "LiveBlogPosting",
];

fn is_article_type(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::String(t) => ARTICLE_TYPES.contains(&t.as_str()),
        serde_json::Value::Array(types) => types.iter().any(is_article_type),
        _ => false,
    }
}

fn json_ld_objects(
    value: serde_json::Value,
    out: &mut Vec<serde_json::Map<String, serde_json::Value>>,
) {
    match value {
        serde_json::Value::Array(items) => items
            .into_iter()
            .for_each(|item| json_ld_objects(item, out)),
        serde_json::Value::Object(mut map) => {
            if let Some(graph) = map.remove("@graph") {
                json_ld_objects(graph, out);
            }
            out.push(map);
        }
        _ => {}
    }
}

fn json_string(value: Option<&serde_json::Value>) -> Option<String> {
    match value? {
        serde_json::Value::String(s) => Some(s.trim().to_owned()).filter(|s| !s.is_empty()),
        serde_json::Value::Array(items) => items.iter().find_map(|v| json_string(Some(v))),
        serde_json::Value::Object(map) => json_string(map.get("name").or_else(|| map.get("url"))),
        _ => None,
    }
}

/// Reads the first JSON-LD article object of the page.
pub fn json_ld_article(document: &Html) -> Option<PageMeta> {
    let sel = selector(r#"script[type="application/ld+json"]"#)?;
    let mut objects = Vec::new();
    for script in document.select(&sel) {
        let raw: String = script.text().collect();
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(raw.trim()) {
            json_ld_objects(value, &mut objects);
        }
    }
    let article = objects
        .into_iter()
        .find(|object| object.get("@type").is_some_and(is_article_type))?;
    Some(PageMeta {
        title: json_string(article.get("headline").or_else(|| article.get("name"))),
        author: json_string(article.get("author")),
        published: json_string(article.get("datePublished")),
        image: json_string(article.get("image").or_else(|| article.get("thumbnailUrl"))),
        json_ld_body: json_string(article.get("articleBody")),
    })
}

/// Metadata from `<meta>` tags, JSON-LD and common byline markup.
pub fn page_meta(document: &Html) -> PageMeta {
    let ld = json_ld_article(document).unwrap_or_default();
    PageMeta {
        title: meta_content(
            document,
            &[
                r#"meta[property="og:title"]"#,
                r#"meta[name="twitter:title"]"#,
                r#"meta[name="dc.title"]"#,
            ],
        )
        .or(ld.title)
        .or_else(|| first_text(document, &["h1", "title"])),
        author: meta_content(
            document,
            &[
                r#"meta[name="author"]"#,
                r#"meta[property="article:author"]"#,
                r#"meta[name="byl"]"#,
                r#"meta[name="dc.creator"]"#,
            ],
        )
        .filter(|a| !a.starts_with("http"))
        .or(ld.author)
        .or_else(|| {
            first_text(
                document,
                &[
                    r#"[rel="author"]"#,
                    r#"[itemprop="author"] [itemprop="name"]"#,
                    ".byline",
                    ".author-name",
                ],
            )
        }),
        published: meta_content(
            document,
            &[
                r#"meta[property="article:published_time"]"#,
                r#"meta[name="pubdate"]"#,
                r#"meta[name="publishdate"]"#,
                r#"meta[name="date"]"#,
                r#"meta[itemprop="datePublished"]"#,
                r#"meta[name="dc.date"]"#,
            ],
        )
        .or(ld.published)
        .or_else(|| {
            let sel = selector("time[datetime]")?;
            document
                .select(&sel)
                .find_map(|el| el.value().attr("datetime").map(str::to_owned))
        }),
        image: meta_content(
            document,
            &[
                r#"meta[property="og:image"]"#,
                r#"meta[name="twitter:image"]"#,
                r#"meta[name="twitter:image:src"]"#,
            ],
        )
        .or(ld.image)
        .or_else(|| {
            let sel = selector(r#"link[rel="image_src"]"#)?;
            document
                .select(&sel)
                .find_map(|el| el.value().attr("href").map(str::to_owned))
        }),
        json_ld_body: ld.json_ld_body,
    }
}

/// Mercury's generic cleaners: chrome, ads and social widgets that pollute
/// the content scorer.
const JUNK_SELECTORS: &[&str] = &[
    "script",
    "style",
    "noscript",
    "iframe",
    "form",
    "button",
    "nav",
    "footer",
    "aside",
    "[role=navigation]",
    "[role=banner]",
    "[role=contentinfo]",
    "[role=complementary]",
    "[aria-hidden=true]",
    "[hidden]",
    ".ad",
    ".ads",
    ".advert",
    ".advertisement",
    "[class*=sponsor]",
    "[class*=share]",
    "[class*=social]",
    "[class*=newsletter]",
    "[class*=subscribe]",
    "[class*=related]",
    "[class*=recommend]",
    "[class*=comment]",
    "[class*=cookie]",
    "[class*=promo]",
    "[class*=paywall]",
    "[id*=comment]",
    "[id*=sidebar]",
    "[class*=sidebar]",
];

/// Removes [`JUNK_SELECTORS`] matches (and their subtrees) and returns the
/// re-serialized document. `<body>`, `<html>` and the main candidates are
/// never removed, and neither is any match that holds the article: one that
/// contains an `<article>` / `articleBody` element or at least half of the
/// page's paragraph text. Substring class matches (`share`, `comment`, ...)
/// otherwise catch wrappers such as `post has-share-bar` and would empty the page.
pub fn strip_junk(html: &str) -> String {
    let mut document = Html::parse_document(html);
    let total = selector("body")
        .and_then(|body| document.select(&body).next())
        .map_or(0, paragraph_chars);
    let holds_article = selector(r#"article, [itemprop="articleBody"]"#);
    let mut doomed = Vec::new();
    for css in JUNK_SELECTORS {
        let Some(sel) = selector(css) else { continue };
        for element in document.select(&sel) {
            let name = element.value().name();
            if matches!(name, "html" | "body" | "main" | "article") {
                continue;
            }
            let wraps_article = holds_article
                .as_ref()
                .is_some_and(|s| element.select(s).next().is_some());
            if wraps_article || (total > 0 && paragraph_chars(element) * 2 >= total) {
                continue;
            }
            doomed.push(element.id());
        }
    }
    for id in doomed {
        if let Some(mut node) = document.tree.get_mut(id) {
            node.detach();
        }
    }
    document.html()
}

/// Total text length of `<p>` descendants (content density proxy).
pub fn paragraph_chars(element: ElementRef<'_>) -> usize {
    let Some(sel) = selector("p") else { return 0 };
    element
        .select(&sel)
        .map(|p| collapse(&p.text().collect::<String>()).chars().count())
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_extraction_matches_the_ts_regex() {
        assert_eq!(
            extract_title_from_html("<html><TITLE>  Hello &amp; bye </TITLE>").as_deref(),
            Some("Hello &amp; bye")
        );
        assert_eq!(extract_title_from_html("<title></title>"), None);
        assert_eq!(extract_title_from_html("<title>a<b></title>"), None);
    }

    #[test]
    fn reads_json_ld_graphs() {
        let html = r#"<html><head><script type="application/ld+json">
            {"@graph":[{"@type":"WebPage"},{"@type":["NewsArticle"],"headline":"H",
             "author":[{"@type":"Person","name":"Ann"}],"datePublished":"2026-01-02",
             "image":{"url":"https://img.test/a.jpg"},"articleBody":"Body text"}]}
        </script></head><body></body></html>"#;
        let meta = json_ld_article(&Html::parse_document(html)).unwrap();
        assert_eq!(meta.title.as_deref(), Some("H"));
        assert_eq!(meta.author.as_deref(), Some("Ann"));
        assert_eq!(meta.published.as_deref(), Some("2026-01-02"));
        assert_eq!(meta.image.as_deref(), Some("https://img.test/a.jpg"));
        assert_eq!(meta.json_ld_body.as_deref(), Some("Body text"));
    }

    #[test]
    fn never_strips_a_junk_looking_wrapper_that_holds_the_article() {
        let body = "Readable paragraph text that carries the actual story forward. ".repeat(4);
        let html = format!(
            r#"<html><body><div class="post has-share-bar"><p>{body}</p><p>{body}</p></div>
            <div class="share-tools"><p>Share this</p></div></body></html>"#
        );
        let cleaned = strip_junk(&html);
        assert!(cleaned.contains("carries the actual story"));
        assert!(!cleaned.contains("Share this"));
    }

    #[test]
    fn strips_chrome_but_keeps_the_article() {
        let html = r#"<html><body><nav>Menu</nav><article class="share-wrapper"><p>Keep</p>
            <div class="social-links">Tweet</div></article><footer>Foot</footer></body></html>"#;
        let cleaned = strip_junk(html);
        assert!(cleaned.contains("Keep"));
        assert!(!cleaned.contains("Menu"));
        assert!(!cleaned.contains("Tweet"));
        assert!(!cleaned.contains("Foot"));
    }
}
