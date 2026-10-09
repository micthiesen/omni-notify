//! Generated research rendered as safe HTML.
//!
//! Mirrors `react-markdown` + `remark-gfm` with `skipHtml`: raw HTML is
//! dropped, text is escaped, link and image URLs pass `defaultUrlTransform`
//! (unsafe destinations are removed), headings shift below the artifact h3,
//! links open in a new tab, and tables sit in a scrollable region.

use leptos::prelude::*;
use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

/// `react-markdown`'s `defaultUrlTransform`: keeps relative URLs and the
/// `http(s)`, `irc(s)`, `mailto` and `xmpp` protocols; anything else is "".
pub fn safe_url(value: &str) -> String {
    let colon = value.find(':');
    let question = value.find('?');
    let hash = value.find('#');
    let slash = value.find('/');
    let Some(colon) = colon else {
        return value.to_owned();
    };
    let before = |other: Option<usize>| other.is_some_and(|o| colon > o);
    let protocol = value[..colon].to_ascii_lowercase();
    if before(slash)
        || before(question)
        || before(hash)
        || matches!(
            protocol.as_str(),
            "http" | "https" | "irc" | "ircs" | "mailto" | "xmpp"
        )
    {
        value.to_owned()
    } else {
        String::new()
    }
}

fn escape_into(out: &mut String, text: &str) {
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            other => out.push(other),
        }
    }
}

fn heading_tag(level: HeadingLevel) -> &'static str {
    match level {
        HeadingLevel::H1 | HeadingLevel::H2 => "h4",
        HeadingLevel::H3 => "h5",
        HeadingLevel::H4 => "h6",
        HeadingLevel::H5 => "h5",
        HeadingLevel::H6 => "h6",
    }
}

fn align_attr(alignment: Option<&Alignment>) -> &'static str {
    match alignment {
        Some(Alignment::Left) => " style=\"text-align:left\"",
        Some(Alignment::Center) => " style=\"text-align:center\"",
        Some(Alignment::Right) => " style=\"text-align:right\"",
        _ => "",
    }
}

/// Renders `content` to the HTML `WorkspaceMarkdown` mounts.
pub fn render_markdown_html(content: &str) -> String {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_GFM
        | Options::ENABLE_FOOTNOTES;
    let mut out = String::with_capacity(content.len() * 2);
    // Close tags for links, which render as `<a>` or `<span>`.
    let mut link_stack: Vec<&'static str> = Vec::new();
    let mut heading_stack: Vec<&'static str> = Vec::new();
    let mut alignments: Vec<Alignment> = Vec::new();
    let mut in_head = false;
    let mut cell = 0usize;
    // Image alt text is collected from the inline events inside the image.
    let mut image: Option<(String, String, String)> = None;

    for event in Parser::new_ext(content, options) {
        if let Some((_, _, alt)) = image.as_mut() {
            match &event {
                Event::Text(text) | Event::Code(text) => {
                    alt.push_str(text);
                    continue;
                }
                Event::End(TagEnd::Image) => {}
                _ => continue,
            }
        }
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => out.push_str("<p>"),
                Tag::Heading { level, .. } => {
                    let tag = heading_tag(level);
                    heading_stack.push(tag);
                    out.push('<');
                    out.push_str(tag);
                    out.push('>');
                }
                Tag::BlockQuote(_) => out.push_str("<blockquote>\n"),
                Tag::CodeBlock(kind) => {
                    out.push_str("<pre><code");
                    if let CodeBlockKind::Fenced(lang) = kind {
                        let lang = lang.split_whitespace().next().unwrap_or("");
                        if !lang.is_empty() {
                            out.push_str(" class=\"language-");
                            escape_into(&mut out, lang);
                            out.push('"');
                        }
                    }
                    out.push('>');
                }
                Tag::List(Some(start)) => {
                    if start == 1 {
                        out.push_str("<ol>\n");
                    } else {
                        out.push_str(&format!("<ol start=\"{start}\">\n"));
                    }
                }
                Tag::List(None) => out.push_str("<ul>\n"),
                Tag::Item => out.push_str("<li>"),
                Tag::FootnoteDefinition(label) => {
                    out.push_str("<div class=\"footnote-definition\"><sup>");
                    escape_into(&mut out, &label);
                    out.push_str("</sup>");
                }
                Tag::Table(aligns) => {
                    alignments = aligns;
                    out.push_str(
                        "<div class=\"workspace-markdown-table\" role=\"region\" \
                         aria-label=\"Research table\" tabindex=\"0\"><table>",
                    );
                }
                Tag::TableHead => {
                    in_head = true;
                    cell = 0;
                    out.push_str("<thead><tr>");
                }
                Tag::TableRow => {
                    cell = 0;
                    out.push_str("<tr>");
                }
                Tag::TableCell => {
                    out.push_str(if in_head { "<th" } else { "<td" });
                    out.push_str(align_attr(alignments.get(cell)));
                    out.push('>');
                }
                Tag::Emphasis => out.push_str("<em>"),
                Tag::Strong => out.push_str("<strong>"),
                Tag::Strikethrough => out.push_str("<del>"),
                Tag::Link {
                    dest_url, title, ..
                } => {
                    let href = safe_url(&dest_url);
                    if href.is_empty() {
                        link_stack.push("</span>");
                        out.push_str("<span>");
                    } else {
                        link_stack.push("</a>");
                        out.push_str("<a href=\"");
                        escape_into(&mut out, &href);
                        out.push('"');
                        if !title.is_empty() {
                            out.push_str(" title=\"");
                            escape_into(&mut out, &title);
                            out.push('"');
                        }
                        out.push_str(" target=\"_blank\" rel=\"noopener noreferrer\">");
                    }
                }
                Tag::Image {
                    dest_url, title, ..
                } => {
                    image = Some((safe_url(&dest_url), title.to_string(), String::new()));
                }
                Tag::HtmlBlock
                | Tag::MetadataBlock(_)
                | Tag::DefinitionList
                | Tag::DefinitionListTitle
                | Tag::DefinitionListDefinition
                | Tag::Superscript
                | Tag::Subscript => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph => out.push_str("</p>\n"),
                TagEnd::Heading(_) => {
                    let tag = heading_stack.pop().unwrap_or("h4");
                    out.push_str("</");
                    out.push_str(tag);
                    out.push_str(">\n");
                }
                TagEnd::BlockQuote(_) => out.push_str("</blockquote>\n"),
                TagEnd::CodeBlock => out.push_str("</code></pre>\n"),
                TagEnd::List(true) => out.push_str("</ol>\n"),
                TagEnd::List(false) => out.push_str("</ul>\n"),
                TagEnd::Item => out.push_str("</li>\n"),
                TagEnd::FootnoteDefinition => out.push_str("</div>\n"),
                TagEnd::Table => out.push_str("</tbody></table></div>\n"),
                TagEnd::TableHead => {
                    in_head = false;
                    out.push_str("</tr></thead><tbody>");
                }
                TagEnd::TableRow => out.push_str("</tr>"),
                TagEnd::TableCell => {
                    out.push_str(if in_head { "</th>" } else { "</td>" });
                    cell += 1;
                }
                TagEnd::Emphasis => out.push_str("</em>"),
                TagEnd::Strong => out.push_str("</strong>"),
                TagEnd::Strikethrough => out.push_str("</del>"),
                TagEnd::Link => out.push_str(link_stack.pop().unwrap_or("</a>")),
                TagEnd::Image => {
                    if let Some((src, title, alt)) = image.take() {
                        out.push_str("<img src=\"");
                        escape_into(&mut out, &src);
                        out.push_str("\" alt=\"");
                        escape_into(&mut out, &alt);
                        out.push('"');
                        if !title.is_empty() {
                            out.push_str(" title=\"");
                            escape_into(&mut out, &title);
                            out.push('"');
                        }
                        out.push_str(" loading=\"lazy\">");
                    }
                }
                _ => {}
            },
            Event::Text(text) => escape_into(&mut out, &text),
            Event::Code(code) => {
                out.push_str("<code>");
                escape_into(&mut out, &code);
                out.push_str("</code>");
            }
            Event::SoftBreak => out.push('\n'),
            Event::HardBreak => out.push_str("<br>\n"),
            Event::Rule => out.push_str("<hr>\n"),
            Event::TaskListMarker(checked) => {
                out.push_str(if checked {
                    "<input type=\"checkbox\" disabled checked> "
                } else {
                    "<input type=\"checkbox\" disabled> "
                });
            }
            Event::FootnoteReference(label) => {
                out.push_str("<sup>");
                escape_into(&mut out, &label);
                out.push_str("</sup>");
            }
            Event::InlineMath(text) | Event::DisplayMath(text) => escape_into(&mut out, &text),
            // skipHtml: raw HTML never renders.
            Event::Html(_) | Event::InlineHtml(_) => {}
        }
    }
    out
}

/// Research markdown inside `.workspace-markdown`.
#[component]
pub fn WorkspaceMarkdown(#[prop(into)] content: Signal<String>) -> impl IntoView {
    let html = Memo::new(move |_| render_markdown_html(&content.get()));
    view! { <div class="workspace-markdown" inner_html=move || html.get()></div> }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_research_comparisons_as_headings_lists_and_semantic_gfm_tables() {
        let html = render_markdown_html(
            "# Shortlist\n\nA **durable** choice for daily use.\n\n- Replaceable battery\n- Two-year warranty\n\n| Model | Price |\n| --- | --- |\n| Example | $20 |\n\n[Manufacturer](https://example.com/product)\n",
        );
        assert!(html.contains("<h4>Shortlist</h4>"), "{html}");
        assert!(html.contains("<strong>durable</strong>"));
        assert!(html.contains("<li>Replaceable battery</li>"));
        assert!(html.contains("<table>"));
        assert!(html.contains("<th>Model</th>"));
        assert!(html.contains("<td>$20</td>"));
        assert!(html.contains("href=\"https://example.com/product\""));
        assert!(html.contains("rel=\"noopener noreferrer\""));
    }

    #[test]
    fn does_not_execute_raw_html_embedded_in_generated_research() {
        let html = render_markdown_html(
            "Useful research.\n\n<script>alert(document.cookie)</script>\n\n<img src=x onerror=\"alert(1)\">\n\n<iframe src=\"https://example.com\"></iframe>\n\nStill useful research.",
        );
        assert!(html.contains("Useful research."));
        assert!(html.contains("Still useful research."));
        assert!(!html.contains("<script"));
        assert!(!html.contains("<img"));
        assert!(!html.contains("<iframe"));
        assert!(!html.contains("onerror"));
    }

    #[test]
    fn keeps_link_text_but_removes_unsafe_destinations() {
        for destination in [
            "javascript:alert%281%29",
            "JaVaScRiPt:alert%281%29",
            "data:text/html;base64,PHNjcmlwdD4=",
            "vbscript:msgbox%281%29",
        ] {
            let html = render_markdown_html(&format!("[Research source]({destination})"));
            assert!(html.contains("Research source"), "{destination}");
            assert!(!html.contains("href="), "{destination}");
            assert!(!html.contains(destination), "{destination}");
        }
    }

    #[test]
    fn safe_url_matches_default_url_transform() {
        assert_eq!(safe_url("/relative:path"), "/relative:path");
        assert_eq!(safe_url("#a:b"), "#a:b");
        assert_eq!(safe_url("mailto:x@y.z"), "mailto:x@y.z");
        assert_eq!(safe_url("javascript:x"), "");
    }
}
