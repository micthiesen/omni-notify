//! Article text formatting (`src/press-pods/formatting/*`).
//!
//! `clean_text` replaces the `html-to-text` call TS made with an in-house
//! converter over `scraper`'s DOM. It keeps what the narration pipeline relies
//! on: links reduced to their text, images, rules and tables dropped, `h1`-`h3`
//! in their original case and `h4`-`h6` uppercased (html-to-text's default,
//! which TS overrode only for `h1`-`h3`), paragraphs separated by a blank line, list items
//! prefixed `" * "` / `"1. "`, and blockquote lines prefixed `"> "` (the cleaner
//! prompt and the RSS show notes both key off that prefix). The output is
//! model input, so exact whitespace parity with `html-to-text` is not a goal
//! (html-to-text drift is accepted for prompts).

use std::sync::LazyLock;

use jiff::tz::TimeZone;
use regex::Regex;
use scraper::{ElementRef, Html, Node};

use crate::error::PressPodsError;

const QUOTE_PREFIX: &str = r"^\s*[>]+\s*";

static QUOTE_PREFIX_RE: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(QUOTE_PREFIX).ok());
static STANDARDIZE_RE: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"^\s*[>][>\s]*").ok());

/// `html-to-text`'s whitespace set (U+00A0 is content, not whitespace).
fn is_html_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\r' | '\n' | '\x0c' | '\u{200b}')
}

/// `cleanText`: HTML to narration-ready plain text. Fails on input shorter than
/// 200 UTF-16 units (`"Article is too short: <input>"`).
pub fn clean_text(dirty: &str) -> Result<String, PressPodsError> {
    if omni_core::js::utf16_len(dirty) < 200 {
        return Err(PressPodsError::failed(
            "clean article text",
            format!("Article is too short: {dirty}"),
        ));
    }
    let text = html_to_text(dirty);
    let lines: Vec<String> = text.split('\n').map(standardize_prefix).collect();
    Ok(remove_extra_empty_lines(&lines).join("\n"))
}

/// Some quoted lines start with `"> > "` instead of `"> "`.
fn standardize_prefix(line: &str) -> String {
    match STANDARDIZE_RE.as_ref() {
        Some(re) => re.replace(line, "> ").into_owned(),
        None => line.to_owned(),
    }
}

struct LineInfo {
    exists: bool,
    is_quote: bool,
    is_empty: bool,
}

fn line_info(line: Option<&String>) -> LineInfo {
    let Some(line) = line else {
        return LineInfo {
            exists: false,
            is_quote: false,
            is_empty: false,
        };
    };
    let (is_quote, clean) = match QUOTE_PREFIX_RE.as_ref() {
        Some(re) => (re.is_match(line), re.replace(line, "").into_owned()),
        None => (false, line.clone()),
    };
    LineInfo {
        exists: true,
        is_quote,
        is_empty: clean.is_empty(),
    }
}

/// `removeExtraEmptyLines`: drops empty quote lines, leading and trailing
/// empty lines, and collapses runs of empty lines between prose to one.
pub fn remove_extra_empty_lines(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .enumerate()
        .filter(|(index, line)| {
            let current = line_info(Some(line));
            if !current.is_empty {
                return true;
            }
            if current.is_quote {
                return false;
            }
            let prev = line_info(index.checked_sub(1).and_then(|i| lines.get(i)));
            let next = line_info(lines.get(index + 1));
            if !prev.exists || !next.exists {
                return false;
            }
            !(!prev.is_quote && next.is_empty)
        })
        .map(|(_, line)| line.clone())
        .collect()
}

/// Inputs to [`build_final_text`].
pub struct FinalTextInput<'a> {
    pub title: Option<&'a str>,
    pub domain: Option<&'a str>,
    pub author: &'a str,
    pub coauthors: &'a [String],
    pub date_published_ms: Option<i64>,
    pub text: &'a str,
    pub tz: &'a TimeZone,
}

/// `buildFinalText`: `"<Title>. By <Author>[ and <co>, <co>]. [Published <Month d, yyyy>[ on <domain>]. ]\n\n<text>"`.
pub fn build_final_text(input: &FinalTextInput<'_>) -> String {
    let mut out = String::new();
    if let Some(title) = input.title.filter(|t| !t.is_empty()) {
        out.push_str(title);
        out.push_str(". ");
    }
    out.push_str("By ");
    out.push_str(input.author);
    if !input.coauthors.is_empty() {
        out.push_str(" and ");
        out.push_str(&input.coauthors.join(", "));
    }
    out.push_str(". ");
    if let Some(date) = input
        .date_published_ms
        .and_then(|ms| format_long_date(ms, input.tz))
    {
        out.push_str("Published ");
        out.push_str(&date);
        if let Some(domain) = input.domain.filter(|d| !d.is_empty()) {
            out.push_str(" on ");
            out.push_str(domain);
        }
        out.push_str(". ");
    }
    out.push_str("\n\n");
    out.push_str(input.text);
    out
}

/// date-fns `format(date, "MMMM d, yyyy")` in the service time zone.
pub fn format_long_date(ms: i64, tz: &TimeZone) -> Option<String> {
    let ts = jiff::Timestamp::from_millisecond(ms).ok()?;
    Some(ts.to_zoned(tz.clone()).strftime("%B %-d, %Y").to_string())
}

// ---------------------------------------------------------------------------
// HTML to text
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Skip,
    Inline,
    /// Block with this many line breaks before and after.
    Block(usize),
    /// `h4`-`h6`: a paragraph-spaced block rendered in upper case.
    UpperHeading,
    Pre,
    Blockquote,
    List {
        ordered: bool,
    },
    ListItem,
    Break,
}

fn kind_of(name: &str) -> Kind {
    match name {
        "script" | "style" | "noscript" | "template" | "head" | "title" | "img" | "hr"
        | "table" | "svg" | "canvas" | "iframe" | "object" | "embed" | "video" | "audio"
        | "picture" | "source" | "math" | "select" | "input" | "button" | "textarea" => Kind::Skip,
        "br" => Kind::Break,
        "p" | "h1" | "h2" | "h3" => Kind::Block(2),
        "h4" | "h5" | "h6" => Kind::UpperHeading,
        "pre" => Kind::Pre,
        "blockquote" => Kind::Blockquote,
        "ul" => Kind::List { ordered: false },
        "ol" => Kind::List { ordered: true },
        "li" => Kind::ListItem,
        "div" | "section" | "article" | "main" | "header" | "footer" | "nav" | "aside"
        | "figure" | "figcaption" | "form" | "fieldset" | "address" | "dl" | "dt" | "dd"
        | "details" | "summary" | "center" | "body" | "html" | "hgroup" | "caption" => {
            Kind::Block(1)
        }
        _ => Kind::Inline,
    }
}

/// Accumulates lines with pending-break bookkeeping and per-line prefixes.
struct TextBuilder {
    out: String,
    pending_breaks: usize,
    line_has_content: bool,
    need_space: bool,
    /// Prefix for the next line started, per nesting level: `(first, rest)`.
    prefixes: Vec<(String, String)>,
    first_line_used: Vec<bool>,
    /// Prefix depth in effect where the pending breaks were requested; blank
    /// lines carry only those prefixes.
    break_depth: usize,
    /// Nesting depth of upper-cased headings.
    uppercase: usize,
}

impl TextBuilder {
    fn new() -> Self {
        Self {
            out: String::new(),
            pending_breaks: 0,
            line_has_content: false,
            need_space: false,
            prefixes: Vec::new(),
            first_line_used: Vec::new(),
            break_depth: usize::MAX,
            uppercase: 0,
        }
    }

    fn request_breaks(&mut self, n: usize) {
        if self.out.is_empty() {
            return;
        }
        self.pending_breaks = self.pending_breaks.max(n);
        self.break_depth = self.break_depth.min(self.prefixes.len());
    }

    fn line_prefix(&mut self) -> String {
        let mut prefix = String::new();
        for (level, (first, rest)) in self.prefixes.iter().enumerate() {
            if self.first_line_used.get(level).copied().unwrap_or(true) {
                prefix.push_str(rest);
            } else {
                prefix.push_str(first);
            }
        }
        for used in &mut self.first_line_used {
            *used = true;
        }
        prefix
    }

    fn start_content(&mut self) {
        if self.pending_breaks > 0 {
            // Blank lines inside a quote carry its marker (html-to-text
            // prefixes every quoted line), so the empty-quote filter drops them.
            let depth = self.break_depth.min(self.prefixes.len());
            self.break_depth = usize::MAX;
            let blank: String = self.prefixes[..depth]
                .iter()
                .map(|(_, rest)| rest.as_str())
                .collect();
            let blank = blank.trim_end();
            for i in 0..self.pending_breaks {
                if i > 0 {
                    self.out.push_str(blank);
                }
                self.out.push('\n');
            }
            self.pending_breaks = 0;
            self.line_has_content = false;
            self.need_space = false;
        }
        if !self.line_has_content {
            let prefix = self.line_prefix();
            self.out.push_str(&prefix);
            self.line_has_content = true;
            self.need_space = false;
        }
    }

    fn word(&mut self, word: &str) {
        let space = self.need_space && self.line_has_content && self.pending_breaks == 0;
        self.start_content();
        if space {
            self.out.push(' ');
        }
        if self.uppercase > 0 {
            self.out.push_str(&word.to_uppercase());
        } else {
            self.out.push_str(word);
        }
        self.need_space = false;
    }

    fn text(&mut self, text: &str) {
        let starts_with_space = text.starts_with(is_html_space);
        let ends_with_space = text.ends_with(is_html_space);
        if starts_with_space {
            self.need_space = true;
        }
        let mut words = text
            .split(is_html_space)
            .filter(|w| !w.is_empty())
            .peekable();
        while let Some(word) = words.next() {
            self.word(word);
            if words.peek().is_some() {
                self.need_space = true;
            }
        }
        if ends_with_space {
            self.need_space = true;
        }
    }

    fn line_break(&mut self) {
        self.break_depth = self.break_depth.min(self.prefixes.len());
        if self.pending_breaks > 0 {
            self.pending_breaks += 1;
        } else if self.line_has_content || !self.out.is_empty() {
            self.pending_breaks = 1;
        }
        self.need_space = false;
    }

    fn preformatted(&mut self, text: &str) {
        for (i, line) in text.split('\n').enumerate() {
            if i > 0 {
                self.pending_breaks = self.pending_breaks.max(1);
            }
            if line.is_empty() {
                continue;
            }
            self.start_content();
            self.out.push_str(line.trim_end_matches('\r'));
        }
    }

    fn push_prefix(&mut self, first: &str, rest: &str) {
        self.prefixes.push((first.to_owned(), rest.to_owned()));
        self.first_line_used.push(false);
    }

    fn pop_prefix(&mut self) {
        self.prefixes.pop();
        self.first_line_used.pop();
    }
}

fn walk(builder: &mut TextBuilder, element: ElementRef<'_>) {
    for child in element.children() {
        match child.value() {
            Node::Text(text) => builder.text(text),
            Node::Element(_) => {
                if let Some(child_element) = ElementRef::wrap(child) {
                    render_element(builder, child_element);
                }
            }
            _ => {}
        }
    }
}

fn pre_text(element: ElementRef<'_>) -> String {
    element.text().collect()
}

fn render_element(builder: &mut TextBuilder, element: ElementRef<'_>) {
    match kind_of(element.value().name()) {
        Kind::Skip => {}
        Kind::Inline => walk(builder, element),
        Kind::Break => builder.line_break(),
        Kind::Block(n) => {
            builder.request_breaks(n);
            walk(builder, element);
            builder.request_breaks(n);
        }
        Kind::UpperHeading => {
            builder.request_breaks(2);
            builder.uppercase += 1;
            walk(builder, element);
            builder.uppercase -= 1;
            builder.request_breaks(2);
        }
        Kind::Pre => {
            builder.request_breaks(2);
            builder.preformatted(&pre_text(element));
            builder.request_breaks(2);
        }
        Kind::Blockquote => {
            builder.request_breaks(2);
            builder.push_prefix("> ", "> ");
            walk(builder, element);
            builder.pop_prefix();
            builder.request_breaks(2);
        }
        Kind::List { ordered } => {
            builder.request_breaks(2);
            let mut number = 1usize;
            for child in element.children() {
                let Some(item) = ElementRef::wrap(child) else {
                    if let Node::Text(text) = child.value()
                        && !text.trim_matches(is_html_space).is_empty()
                    {
                        builder.text(text);
                    }
                    continue;
                };
                if item.value().name() == "li" {
                    let marker = if ordered {
                        format!("{number}. ")
                    } else {
                        " * ".to_owned()
                    };
                    number += 1;
                    let indent = " ".repeat(marker.chars().count());
                    builder.request_breaks(1);
                    builder.push_prefix(&marker, &indent);
                    walk(builder, item);
                    builder.pop_prefix();
                    builder.request_breaks(1);
                } else {
                    render_element(builder, item);
                }
            }
            builder.request_breaks(2);
        }
        Kind::ListItem => {
            builder.request_breaks(1);
            builder.push_prefix(" * ", "   ");
            walk(builder, element);
            builder.pop_prefix();
            builder.request_breaks(1);
        }
    }
}

/// Renders the document body (or the whole fragment) as plain text.
pub fn html_to_text(html: &str) -> String {
    let document = Html::parse_document(html);
    let mut builder = TextBuilder::new();
    let body = scraper::Selector::parse("body")
        .ok()
        .and_then(|selector| document.select(&selector).next());
    match body {
        Some(body) => walk(&mut builder, body),
        None => walk(&mut builder, document.root_element()),
    }
    builder.out
}

#[cfg(test)]
mod line_filtering_spec {
    //! Ports `src/press-pods/formatting/lineFiltering.spec.ts`.
    use super::remove_extra_empty_lines;

    fn lines(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn keeps_non_empty_lines() {
        assert_eq!(
            remove_extra_empty_lines(&lines(&["a", "b"])),
            lines(&["a", "b"])
        );
    }

    #[test]
    fn drops_empty_quote_lines() {
        assert_eq!(
            remove_extra_empty_lines(&lines(&["> a", "> ", "> b"])),
            lines(&["> a", "> b"])
        );
    }

    #[test]
    fn drops_leading_and_trailing_empty_lines() {
        assert_eq!(
            remove_extra_empty_lines(&lines(&["", "a", ""])),
            lines(&["a"])
        );
    }

    #[test]
    fn collapses_runs_of_empty_lines_between_prose() {
        assert_eq!(
            remove_extra_empty_lines(&lines(&["a", "", "", "b"])),
            lines(&["a", "", "b"])
        );
    }

    #[test]
    fn keeps_a_single_separator_between_paragraphs() {
        assert_eq!(
            remove_extra_empty_lines(&lines(&["a", "", "b"])),
            lines(&["a", "", "b"])
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn long(body: &str) -> String {
        format!(
            "<html><head><title>T</title><script>var x = 1;</script></head><body>{body}<p>{}</p></body></html>",
            "padding ".repeat(30)
        )
    }

    #[test]
    fn rejects_short_input() {
        let error = clean_text("<p>tiny</p>").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Article is too short: <p>tiny</p>")
        );
    }

    #[test]
    fn renders_paragraphs_links_quotes_and_lists() {
        let html = long(
            "<h1>Big Title</h1><p>First <a href=\"https://x.test\">linked</a>  words.</p>\
             <img src=\"a.png\"><hr><table><tr><td>cell</td></tr></table>\
             <blockquote><p>Quoted line</p><blockquote><p>Nested</p></blockquote></blockquote>\
             <ul><li>one</li><li>two</li></ul><ol><li>first</li></ol><p>a<br>b</p>",
        );
        let text = clean_text(&html).unwrap();
        let expected_start = "Big Title\n\nFirst linked words.\n\n> Quoted line\n> Nested\n\n * one\n * two\n\n1. first\n\na\nb\n\npadding";
        assert!(text.starts_with(expected_start), "{text:?}");
        assert!(!text.contains("cell"));
        assert!(!text.contains("var x"));
        assert!(!text.contains("https://x.test"));
    }

    #[test]
    fn uppercases_minor_headings_like_html_to_text() {
        let text = clean_text(&long("<h2>Major Point</h2><h4>Minor <em>point</em></h4>")).unwrap();
        assert!(
            text.starts_with("Major Point\n\nMINOR POINT\n\npadding"),
            "{text:?}"
        );
    }

    #[test]
    fn keeps_non_breaking_spaces_and_decodes_entities() {
        let text = clean_text(&long("<p>A&nbsp;B &amp; C</p>")).unwrap();
        assert!(text.starts_with("A\u{a0}B & C"), "{text:?}");
    }

    #[test]
    fn builds_the_spoken_header_line() {
        let tz = TimeZone::get("America/Vancouver").unwrap();
        let text = build_final_text(&FinalTextInput {
            title: Some("Title"),
            domain: Some("example.com"),
            author: "Jane",
            coauthors: &["Ann".to_owned(), "Bo".to_owned()],
            date_published_ms: Some(1_784_030_671_000),
            text: "Body",
            tz: &tz,
        });
        assert_eq!(
            text,
            "Title. By Jane and Ann, Bo. Published July 14, 2026 on example.com. \n\nBody"
        );
        let bare = build_final_text(&FinalTextInput {
            title: None,
            domain: Some("example.com"),
            author: "Anonymous",
            coauthors: &[],
            date_published_ms: None,
            text: "Body",
            tz: &tz,
        });
        assert_eq!(bare, "By Anonymous. \n\nBody");
    }
}
