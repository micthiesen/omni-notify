//! HTML bodies to plain text (`src/email/htmlToText.ts`).
//!
//! [`html_to_text`] ports the html-to-text 10 block/inline text builder for the
//! exact options TS uses: `wordwrap: false`, entities decoded, anchors without
//! hrefs, images and horizontal rules skipped, h1-h3 not uppercased (h4-h6
//! are, the library default), and tables rendered as plain blocks (no data
//! tables are configured, so `colSpacing` never applies). With wrapping off,
//! word-wrap and `<wbr>` opportunities have no effect.
//!
//! Parsing uses html5ever, which (unlike htmlparser2) always synthesizes
//! `html`/`head`/`body`; when the source has no `<body` tag the whole
//! document is walked, as html-to-text does without a `body` base element.
//! Tree-construction differences on malformed markup are the remaining
//! divergence.

use std::sync::LazyLock;

use ego_tree::NodeRef;
use omni_core::js::{number_to_string, string_to_number, utf16_len, utf16_slice};
use regex::Regex;
use scraper::{Html, Node};

const MAX_INPUT_LENGTH: usize = 1 << 24;
const MAX_LINKS: usize = 20;
const MAX_LINK_LENGTH: usize = 500;

#[allow(clippy::expect_used)]
static LINK_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)href\s*=\s*["']([^"']+)["']"#).expect("static regex"));
#[allow(clippy::expect_used)]
static INTERESTING_LINK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)track|shipment|deliver|parcel|order|booking|reservation")
        .expect("static regex")
});
#[allow(clippy::expect_used)]
static BODY_TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)<body[\s>/]").expect("static regex"));

/// `extractInterestingLinks`: shipment/booking-shaped hrefs (tracking numbers
/// often exist only inside link URLs, which the text conversion drops).
pub fn extract_interesting_links(html: &str) -> Vec<String> {
    let mut links: Vec<String> = Vec::new();
    for captures in LINK_PATTERN.captures_iter(html) {
        let Some(url) = captures.get(1).map(|m| m.as_str()) else {
            continue;
        };
        if !url.starts_with("http")
            || utf16_len(url) > MAX_LINK_LENGTH
            || !INTERESTING_LINK.is_match(url)
            || links.iter().any(|seen| seen == url)
        {
            continue;
        }
        links.push(url.to_owned());
        if links.len() >= MAX_LINKS {
            break;
        }
    }
    links
}

/// html-to-text's `whitespaceCharacters` (`' \t\r\n\f​'`).
fn is_html_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\r' | '\n' | '\u{000C}' | '\u{200B}')
}

/// `InlineTextBuilder` with wrapping disabled.
#[derive(Default)]
struct Inline {
    lines: Vec<Vec<String>>,
    next_line_words: Vec<String>,
    stashed_space: bool,
}

impl Inline {
    fn push_word(&mut self, word: String) {
        self.next_line_words.push(word);
    }

    fn concat_word(&mut self, word: &str) {
        match self.next_line_words.pop() {
            Some(mut last) => {
                last.push_str(word);
                self.next_line_words.push(last);
            }
            None => self.next_line_words.push(word.to_owned()),
        }
    }

    fn start_new_line(&mut self, n: usize) {
        self.lines.push(std::mem::take(&mut self.next_line_words));
        for _ in 1..n {
            self.lines.push(Vec::new());
        }
    }

    fn is_empty(&self) -> bool {
        self.lines.is_empty() && self.next_line_words.is_empty()
    }

    fn clear(&mut self) {
        self.lines.clear();
        self.next_line_words.clear();
    }

    fn render(&self) -> String {
        self.lines
            .iter()
            .chain(std::iter::once(&self.next_line_words))
            .map(|words| words.join(" "))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// `WhitespaceProcessor.shrinkWrapAdd` (`preserveNewlines: false`).
    fn shrink_wrap_add(&mut self, text: &str, transform: Option<&dyn Fn(&str) -> String>) {
        if text.is_empty() {
            return;
        }
        let apply = |word: &str| transform.map_or_else(|| word.to_owned(), |t| t(word));
        let previously_stashed = self.stashed_space;
        let mut words = text.split(is_html_space).filter(|w| !w.is_empty());
        let any = match words.next() {
            Some(first) => {
                if previously_stashed || text.starts_with(is_html_space) {
                    self.push_word(apply(first));
                } else {
                    self.concat_word(&apply(first));
                }
                for word in words {
                    self.push_word(apply(word));
                }
                true
            }
            None => false,
        };
        self.stashed_space = (previously_stashed && !any) || text.ends_with(is_html_space);
    }
}

#[derive(Clone, PartialEq, Eq)]
enum Kind {
    Block,
    List {
        max_prefix_length: usize,
        inter_row_line_breaks: usize,
    },
    ListItem {
        prefix: String,
    },
}

/// `BlockStackItem` and its list subclasses.
struct Item {
    kind: Kind,
    leading_line_breaks: usize,
    inline: Inline,
    raw_text: String,
    stashed_line_breaks: usize,
    is_pre: bool,
}

impl Item {
    fn new(kind: Kind, leading_line_breaks: usize, is_pre: bool) -> Self {
        Self {
            kind,
            leading_line_breaks,
            inline: Inline::default(),
            raw_text: String::new(),
            stashed_line_breaks: 0,
            is_pre,
        }
    }

    /// `getText`.
    fn text(&self) -> String {
        if self.inline.is_empty() {
            self.raw_text.clone()
        } else {
            format!("{}{}", self.raw_text, self.inline.render())
        }
    }

    /// `addText`.
    fn add_text(&mut self, text: &str, leading: usize, trailing: usize) {
        let parent_text = self.text();
        let line_breaks = self.stashed_line_breaks.max(leading);
        self.inline.clear();
        if parent_text.is_empty() {
            self.raw_text = text.to_owned();
            self.leading_line_breaks = line_breaks;
        } else {
            self.raw_text = format!("{parent_text}{}{text}", "\n".repeat(line_breaks));
        }
        self.stashed_line_breaks = trailing;
    }
}

/// `BlockTextBuilder` (only the block, list and inline operations the
/// configured formatters use).
struct Builder {
    stack: Vec<Item>,
    uppercase_depth: usize,
}

impl Builder {
    fn new() -> Self {
        Self {
            stack: vec![Item::new(Kind::Block, 1, false)],
            uppercase_depth: 0,
        }
    }

    fn top(&mut self) -> &mut Item {
        let last = self.stack.len() - 1;
        &mut self.stack[last]
    }

    fn pop(&mut self) -> Item {
        // The root is never popped: every open has a matching close.
        if self.stack.len() > 1 {
            self.stack
                .pop()
                .unwrap_or_else(|| Item::new(Kind::Block, 1, false))
        } else {
            Item::new(Kind::Block, 1, false)
        }
    }

    fn add_inline(&mut self, text: &str) {
        let uppercase = self.uppercase_depth > 0;
        let item = self.top();
        if item.is_pre {
            item.raw_text.push_str(text);
            return;
        }
        let contains_words = text.chars().any(|c| !is_html_space(c));
        if text.is_empty() || (item.stashed_line_breaks > 0 && !contains_words) {
            return;
        }
        if item.stashed_line_breaks > 0 {
            item.inline.start_new_line(item.stashed_line_breaks);
        }
        let upper = |word: &str| word.to_uppercase();
        let transform: Option<&dyn Fn(&str) -> String> =
            if uppercase { Some(&upper) } else { None };
        item.inline.shrink_wrap_add(text, transform);
        item.stashed_line_breaks = 0;
    }

    fn add_line_break(&mut self) {
        let item = self.top();
        if item.is_pre {
            item.raw_text.push('\n');
        } else {
            item.inline.start_new_line(1);
        }
    }

    fn open_block(&mut self, leading_line_breaks: usize, is_pre: bool) {
        let parent_pre = self.top().is_pre;
        self.stack.push(Item::new(
            Kind::Block,
            leading_line_breaks,
            parent_pre || is_pre,
        ));
    }

    fn close_block(
        &mut self,
        trailing_line_breaks: usize,
        transform: Option<&dyn Fn(String) -> String>,
    ) {
        let block = self.pop();
        let text = block.text();
        let text = match transform {
            Some(transform) => transform(text),
            None => text,
        };
        let trailing = block.stashed_line_breaks.max(trailing_line_breaks);
        self.top()
            .add_text(&text, block.leading_line_breaks, trailing);
    }

    fn open_list(&mut self, leading_line_breaks: usize, max_prefix_length: usize) {
        let parent_pre = self.top().is_pre;
        self.stack.push(Item::new(
            Kind::List {
                max_prefix_length,
                inter_row_line_breaks: 1,
            },
            leading_line_breaks,
            parent_pre,
        ));
    }

    fn open_list_item(&mut self, prefix: String) {
        let (inter_row, parent_pre) = match &self.top().kind {
            Kind::List {
                inter_row_line_breaks,
                ..
            } => (*inter_row_line_breaks, self.top().is_pre),
            _ => (1, self.top().is_pre),
        };
        self.stack
            .push(Item::new(Kind::ListItem { prefix }, inter_row, parent_pre));
    }

    fn close_list_item(&mut self) {
        let item = self.pop();
        let prefix = match &item.kind {
            Kind::ListItem { prefix } => prefix.clone(),
            _ => String::new(),
        };
        let (max_prefix_length, inter_row) = match &self.top().kind {
            Kind::List {
                max_prefix_length,
                inter_row_line_breaks,
            } => (*max_prefix_length, *inter_row_line_breaks),
            _ => (0, 1),
        };
        let prefix_length = utf16_len(&prefix).max(max_prefix_length);
        let spacing = format!("\n{}", " ".repeat(prefix_length));
        let padded = format!("{prefix}{}", " ".repeat(prefix_length - utf16_len(&prefix)));
        let text = format!("{padded}{}", item.text().replace('\n', &spacing));
        let trailing = item.stashed_line_breaks.max(inter_row);
        self.top()
            .add_text(&text, item.leading_line_breaks, trailing);
    }

    fn close_list(&mut self, trailing_line_breaks: usize) {
        let list = self.pop();
        let text = list.text();
        if !text.is_empty() {
            self.top()
                .add_text(&text, list.leading_line_breaks, trailing_line_breaks);
        }
    }

    fn finish(mut self) -> String {
        self.stack.truncate(1);
        self.stack.first().map(Item::text).unwrap_or_default()
    }
}

/// `trimCharacter(str, '\n')`.
fn trim_newlines(s: &str) -> &str {
    s.trim_matches('\n')
}

/// `numberToLetterSequence`.
fn letter_sequence(num: i64, base_char: char) -> String {
    let mut digits = Vec::new();
    let mut n = num;
    loop {
        n -= 1;
        digits.push(n.rem_euclid(26));
        n /= 26;
        if n <= 0 {
            break;
        }
    }
    digits
        .iter()
        .rev()
        .filter_map(|d| u32::try_from(*d).ok())
        .filter_map(|d| char::from_u32(base_char as u32 + d))
        .collect()
}

/// `numberToRoman` (1..=3999).
fn roman(num: i64) -> String {
    const I: [&str; 4] = ["I", "X", "C", "M"];
    const V: [&str; 3] = ["V", "L", "D"];
    let digits: Vec<i64> = num
        .to_string()
        .chars()
        .filter_map(|c| c.to_digit(10).map(i64::from))
        .collect();
    let mut parts: Vec<String> = digits
        .iter()
        .rev()
        .enumerate()
        .map(|(i, &v)| {
            let one = I.get(i).copied().unwrap_or("");
            let five = V.get(i).copied().unwrap_or("");
            let ten = I.get(i + 1).copied().unwrap_or("");
            let reps = usize::try_from(v % 5).unwrap_or(0);
            if v % 5 < 4 {
                format!("{}{}", if v < 5 { "" } else { five }, one.repeat(reps))
            } else {
                format!("{one}{}", if v < 5 { five } else { ten })
            }
        })
        .collect();
    parts.reverse();
    parts.concat()
}

fn ordered_index(index: f64, ol_type: Option<&str>) -> String {
    #[allow(clippy::cast_possible_truncation)]
    let whole = (index.is_finite() && index.fract() == 0.0 && index >= 1.0).then_some(index as i64);
    match (ol_type, whole) {
        (Some("a"), Some(n)) => letter_sequence(n, 'a'),
        (Some("A"), Some(n)) => letter_sequence(n, 'A'),
        (Some("i"), Some(n)) => roman(n).to_lowercase(),
        (Some("I"), Some(n)) => roman(n),
        _ => number_to_string(index),
    }
}

fn element_name<'a>(node: &NodeRef<'a, Node>) -> Option<&'a str> {
    node.value().as_element().map(|e| e.name())
}

fn walk_children(node: NodeRef<'_, Node>, builder: &mut Builder) {
    for child in node.children() {
        walk(child, builder);
    }
}

/// `recursiveWalk`: text nodes are added inline, elements formatted; comments,
/// doctypes, `script` and `style` (not `tag` nodes in htmlparser2) are skipped.
fn walk(node: NodeRef<'_, Node>, builder: &mut Builder) {
    match node.value() {
        Node::Text(text) => builder.add_inline(text),
        Node::Element(element) => format_element(node, element.name(), builder),
        Node::Document | Node::Fragment => walk_children(node, builder),
        _ => {}
    }
}

fn format_element(node: NodeRef<'_, Node>, name: &str, builder: &mut Builder) {
    match name {
        "script" | "style" | "img" | "hr" => {}
        "article" | "aside" | "div" | "footer" | "form" | "header" | "main" | "nav" | "section" => {
            builder.open_block(1, false);
            walk_children(node, builder);
            builder.close_block(1, None);
        }
        "p" | "table" => {
            builder.open_block(2, false);
            walk_children(node, builder);
            builder.close_block(2, None);
        }
        "h1" | "h2" | "h3" => {
            builder.open_block(3, false);
            walk_children(node, builder);
            builder.close_block(2, None);
        }
        "h4" | "h5" | "h6" => {
            builder.open_block(2, false);
            builder.uppercase_depth += 1;
            walk_children(node, builder);
            builder.uppercase_depth -= 1;
            builder.close_block(2, None);
        }
        "blockquote" => {
            builder.open_block(2, false);
            walk_children(node, builder);
            let quote = |text: String| {
                trim_newlines(&text)
                    .split('\n')
                    .map(|line| format!("> {line}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            builder.close_block(2, Some(&quote));
        }
        "pre" => {
            builder.open_block(2, true);
            walk_children(node, builder);
            builder.close_block(2, None);
        }
        "br" => builder.add_line_break(),
        "ul" => format_list(node, builder, |_| " * ".to_owned()),
        "ol" => {
            let element = node.value().as_element();
            let start = element
                .and_then(|e| e.attr("start"))
                .filter(|s| !s.is_empty())
                .map_or(1.0, string_to_number);
            let ol_type = element.and_then(|e| e.attr("type")).map(str::to_owned);
            format_list(node, builder, |i| {
                #[allow(clippy::cast_precision_loss)]
                let index = start + i as f64;
                format!(" {}. ", ordered_index(index, ol_type.as_deref()))
            });
        }
        // `wbr` only marks a wrap opportunity, and wrapping is disabled.
        _ => walk_children(node, builder),
    }
}

/// `formatList`.
fn format_list(
    node: NodeRef<'_, Node>,
    builder: &mut Builder,
    prefix_at: impl Fn(usize) -> String,
) {
    let is_nested = node
        .parent()
        .as_ref()
        .and_then(element_name)
        .is_some_and(|name| name == "li");
    let mut index = 0;
    let mut max_prefix_length = 0;
    let mut items: Vec<(NodeRef<'_, Node>, String)> = Vec::new();
    for child in node.children() {
        if let Node::Text(text) = child.value()
            && text.chars().all(char::is_whitespace)
        {
            continue;
        }
        if element_name(&child) != Some("li") {
            items.push((child, String::new()));
            continue;
        }
        let raw = prefix_at(index);
        index += 1;
        let prefix = if is_nested {
            raw.trim_start().to_owned()
        } else {
            raw
        };
        max_prefix_length = max_prefix_length.max(utf16_len(&prefix));
        items.push((child, prefix));
    }
    if items.is_empty() {
        return;
    }
    builder.open_list(if is_nested { 1 } else { 2 }, max_prefix_length);
    for (child, prefix) in items {
        builder.open_list_item(prefix);
        walk(child, builder);
        builder.close_list_item();
    }
    builder.close_list(if is_nested { 1 } else { 2 });
}

/// `htmlToText`.
pub fn html_to_text(html: &str) -> String {
    let html = if utf16_len(html) > MAX_INPUT_LENGTH {
        utf16_slice(html, 0, MAX_INPUT_LENGTH)
    } else {
        std::borrow::Cow::Borrowed(html)
    };
    let document = Html::parse_document(&html);
    let mut builder = Builder::new();
    let root = document.tree.root();
    let body = BODY_TAG
        .is_match(&html)
        .then(|| root.descendants().find(|n| element_name(n) == Some("body")))
        .flatten();
    match body {
        Some(body) => walk(body, &mut builder),
        None => walk(root, &mut builder),
    }
    builder.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapses_whitespace_and_separates_blocks() {
        assert_eq!(
            html_to_text("<p>Hello   <b>big</b>\n world</p><p>Second</p>"),
            "Hello big world\n\nSecond"
        );
        assert_eq!(html_to_text("<div>a</div><div>b</div>"), "a\nb");
        assert_eq!(html_to_text("line<br>next"), "line\nnext");
    }

    #[test]
    fn drops_hrefs_images_and_rules() {
        assert_eq!(
            html_to_text(
                r#"<a href="https://x.test/track">Track</a> <img src="p.png" alt="logo"><hr>now"#
            ),
            "Track now"
        );
    }

    #[test]
    fn keeps_h1_case_and_uppercases_h4() {
        assert_eq!(html_to_text("<h1>Order</h1><h4>Note</h4>"), "Order\n\nNOTE");
    }

    #[test]
    fn formats_lists_and_quotes() {
        assert_eq!(
            html_to_text("<ul><li>one</li><li>two</li></ul>"),
            " * one\n * two"
        );
        assert_eq!(html_to_text("<ol start=\"3\"><li>c</li></ol>"), " 3. c");
        assert_eq!(html_to_text("<blockquote>q</blockquote>"), "> q");
        assert_eq!(roman(1994), "MCMXCIV");
        assert_eq!(letter_sequence(28, 'a'), "ab");
    }

    #[test]
    fn skips_head_when_body_present_and_scripts_always() {
        assert_eq!(
            html_to_text(
                "<html><head><title>T</title><style>p{}</style></head><body><script>x()</script>Hi</body></html>"
            ),
            "Hi"
        );
        assert_eq!(html_to_text("<title>T</title>Hi"), "THi");
    }

    #[test]
    fn extracts_interesting_links_once() {
        let html = r#"<a href="https://shop.test/track/1">a</a><a HREF='https://shop.test/track/1'>b</a>
            <a href="https://shop.test/about">c</a><a href="mailto:order@x">d</a>"#;
        assert_eq!(
            extract_interesting_links(html),
            ["https://shop.test/track/1"]
        );
    }
}
