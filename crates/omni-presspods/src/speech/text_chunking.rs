//! Narration sections and TTS-sized chunks.
//!
//! Lengths are JS string lengths (UTF-16 units), so chunk boundaries and
//! checkpoint keys match existing checkpoints.

use std::sync::LazyLock;

use omni_core::js::utf16_len;
use regex::Regex;

static SECTION_HEADING: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"^##\s+(.+?)\s*$").ok());
static PARAGRAPH_BREAK: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"\n\s*\n").ok());

/// A narration section: an optional `## Heading` title and its body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NarrationSection {
    pub title: Option<String>,
    pub body: String,
}

/// Splits narration on `## Heading` lines (consumed: they become chapter
/// titles and are never spoken). Text before the first heading is the
/// untitled intro; empty sections are dropped.
pub fn split_sections(text: &str) -> Vec<NarrationSection> {
    let mut sections = Vec::new();
    let mut title: Option<String> = None;
    let mut buf: Vec<&str> = Vec::new();
    let flush =
        |title: &Option<String>, buf: &mut Vec<&str>, sections: &mut Vec<NarrationSection>| {
            let body = buf.join("\n").trim().to_owned();
            if !body.is_empty() {
                sections.push(NarrationSection {
                    title: title.clone(),
                    body,
                });
            }
            buf.clear();
        };
    for line in text.split('\n') {
        let heading = SECTION_HEADING
            .as_ref()
            .and_then(|re| re.captures(line))
            .and_then(|c| c.get(1));
        match heading {
            Some(heading) => {
                flush(&title, &mut buf, &mut sections);
                title = Some(heading.as_str().trim().to_owned());
            }
            None => buf.push(line),
        }
    }
    flush(&title, &mut buf, &mut sections);
    if sections.is_empty() {
        sections.push(NarrationSection {
            title: None,
            body: text.trim().to_owned(),
        });
    }
    sections
}

/// `paragraph.split(/(?<=[.!?…])\s+/)` without empty pieces.
fn split_sentences(paragraph: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current = String::new();
    let mut chars = paragraph.chars().peekable();
    while let Some(c) = chars.next() {
        current.push(c);
        if matches!(c, '.' | '!' | '?' | '…') && chars.peek().is_some_and(|n| n.is_whitespace()) {
            while chars.peek().is_some_and(|n| n.is_whitespace()) {
                chars.next();
            }
            sentences.push(std::mem::take(&mut current));
        }
    }
    sentences.push(current);
    sentences
        .into_iter()
        .filter(|s| !s.trim().is_empty())
        .collect()
}

/// Breaks a section into chunks on paragraph boundaries, falling back to
/// sentence boundaries for oversized paragraphs. Never splits mid-sentence (a
/// mid-sentence cut makes the model produce two terminal contours and a
/// spurious pause at the seam).
pub fn chunk_text(text: &str, target: usize, max: usize) -> Vec<String> {
    let paragraphs: Vec<String> = match PARAGRAPH_BREAK.as_ref() {
        Some(re) => re
            .split(text)
            .map(|p| p.trim().to_owned())
            .filter(|p| !p.is_empty())
            .collect(),
        None => vec![text.trim().to_owned()],
    };

    let mut units: Vec<String> = Vec::new();
    for paragraph in paragraphs {
        if utf16_len(&paragraph) <= max {
            units.push(paragraph);
            continue;
        }
        let mut buf = String::new();
        for sentence in split_sentences(&paragraph) {
            if !buf.is_empty() && utf16_len(&buf) + utf16_len(&sentence) + 1 > target {
                units.push(std::mem::replace(&mut buf, sentence));
            } else if buf.is_empty() {
                buf = sentence;
            } else {
                buf.push(' ');
                buf.push_str(&sentence);
            }
        }
        if !buf.is_empty() {
            units.push(buf);
        }
    }

    let mut chunks = Vec::new();
    let mut buf = String::new();
    for unit in units {
        if !buf.is_empty() && utf16_len(&buf) + utf16_len(&unit) + 2 > target {
            chunks.push(std::mem::replace(&mut buf, unit));
        } else if buf.is_empty() {
            buf = unit;
        } else {
            buf.push_str("\n\n");
            buf.push_str(&unit);
        }
    }
    if !buf.is_empty() {
        chunks.push(buf);
    }
    chunks
}

/// Successively smaller targets for adaptive re-splitting; one entry per
/// allowed level.
const RETRY_CHUNK_TARGETS: [usize; 2] = [400, 200];

/// A smaller boundary-safe split for re-split level `depth`, or `None` at the
/// depth cap or when the text cannot actually be split (a long single
/// sentence stays intact).
pub fn split_chunk_for_retry(text: &str, depth: usize) -> Option<Vec<String>> {
    let target = *RETRY_CHUNK_TARGETS.get(depth)?;
    let chunks = chunk_text(text, target, target);
    (chunks.len() > 1).then_some(chunks)
}

#[cfg(test)]
mod text_chunking_spec {
    //! Section and chunk splitting cases.
    use super::*;

    fn section(title: Option<&str>, body: &str) -> NarrationSection {
        NarrationSection {
            title: title.map(str::to_owned),
            body: body.to_owned(),
        }
    }

    fn squash(s: &str) -> String {
        s.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn returns_one_untitled_section_when_there_are_no_headings() {
        assert_eq!(
            split_sections("Just some narration.\n\nA second paragraph."),
            vec![section(None, "Just some narration.\n\nA second paragraph.")]
        );
    }

    #[test]
    fn splits_on_headings_and_keeps_the_intro_untitled() {
        let text = "Opening hook.\n\n## Background\n\nBody one.\n\n## The Turn\n\nBody two.";
        assert_eq!(
            split_sections(text),
            vec![
                section(None, "Opening hook."),
                section(Some("Background"), "Body one."),
                section(Some("The Turn"), "Body two."),
            ]
        );
    }

    #[test]
    fn handles_a_leading_heading_with_no_intro_text() {
        assert_eq!(
            split_sections("## First\n\nBody."),
            vec![section(Some("First"), "Body.")]
        );
    }

    #[test]
    fn never_emits_an_empty_body_section() {
        assert_eq!(
            split_sections("## Empty\n\n## Real\n\nText."),
            vec![section(Some("Real"), "Text.")]
        );
    }

    #[test]
    fn keeps_a_short_text_as_a_single_chunk() {
        assert_eq!(chunk_text("Short.", 900, 1500), vec!["Short."]);
    }

    #[test]
    fn merges_paragraphs_up_to_the_target_and_starts_a_new_chunk_past_it() {
        let p = "A".repeat(300);
        let q = "B".repeat(300);
        let r = "C".repeat(300);
        let chunks = chunk_text(&format!("{p}\n\n{q}\n\n{r}"), 900, 1500);
        assert_eq!(chunks, vec![format!("{p}\n\n{q}"), r]);
    }

    #[test]
    fn splits_an_oversized_paragraph_on_sentence_boundaries_never_mid_sentence() {
        let sentence = format!("{}.", "word ".repeat(40).trim());
        let paragraph = format!("{sentence} {sentence} {sentence}");
        let chunks = chunk_text(&paragraph, 300, 400);
        assert!(chunks.len() > 1);
        for chunk in &chunks {
            assert!(chunk.trim().ends_with('.'));
        }
        assert_eq!(squash(&chunks.join(" ")), squash(&paragraph));
    }

    fn sentence(word: &str, count: usize) -> String {
        format!("{}.", format!("{word} ").repeat(count).trim())
    }

    #[test]
    fn allows_two_successively_smaller_boundary_safe_re_split_levels() {
        let sentences: Vec<String> = (0..6).map(|i| sentence(&format!("word{i}"), 25)).collect();
        let original = sentences.join(" ");
        let first = split_chunk_for_retry(&original, 0).unwrap();
        assert!(first.len() > 1);
        let child = first
            .iter()
            .find(|c| split_chunk_for_retry(c, 1).is_some())
            .unwrap();
        let second = split_chunk_for_retry(child, 1).unwrap();
        assert!(second.len() > 1);
        assert!(split_chunk_for_retry(child, 2).is_none());
        assert_eq!(squash(&first.join(" ")), squash(&original));
        assert_eq!(squash(&second.join(" ")), squash(child));
    }

    #[test]
    fn does_not_claim_a_long_single_sentence_is_splittable() {
        let long = sentence("word", 140);
        assert!(long.len() > 500);
        assert!(split_chunk_for_retry(&long, 0).is_none());
    }

    #[test]
    fn splits_sentences_like_the_lookbehind_regex() {
        assert_eq!(
            split_sentences("One. Two!  Three?\nFour… five"),
            vec!["One.", "Two!", "Three?", "Four…", "five"]
        );
        assert_eq!(split_sentences("No break.Here"), vec!["No break.Here"]);
    }
}
