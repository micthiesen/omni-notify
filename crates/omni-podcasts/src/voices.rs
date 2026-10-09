//! Followed voices from the taste seed's `## Voices` section.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;

static SECTION_HEADER: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"(?i)^##\s+voices\b").ok());
static LIST_ITEM: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"^\s*[-*]\s+(.+)$").ok());
static TRAILING_PAREN: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"\s*\([^()]*\)\s*$").ok());
static TRAILING_ANNOTATION: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"\s*[—:]\s*[^—:]*$").ok());

/// `##` but not `###` (JS `/^##(?!#)/`).
fn is_section_header(line: &str) -> bool {
    line.starts_with("##") && !line[2..].starts_with('#')
}

fn strip(re: &LazyLock<Option<Regex>>, text: &str) -> String {
    match re.as_ref() {
        Some(re) => re.replace(text, "").into_owned(),
        None => text.to_owned(),
    }
}

/// Person names listed under `## Voices ...`, de-duplicated case-insensitively
/// (first casing wins), with trailing parentheticals/annotations and
/// `<placeholder>` items dropped.
pub fn parse_voices(markdown: &str) -> Vec<String> {
    let lines: Vec<&str> = markdown.split('\n').collect();
    let Some(start) = lines
        .iter()
        .position(|line| SECTION_HEADER.as_ref().is_some_and(|re| re.is_match(line)))
    else {
        return Vec::new();
    };
    let mut seen = HashSet::new();
    let mut names = Vec::new();
    for line in &lines[start + 1..] {
        if is_section_header(line) {
            break;
        }
        let Some(raw) = LIST_ITEM
            .as_ref()
            .and_then(|re| re.captures(line))
            .and_then(|caps| caps.get(1))
            .map(|m| m.as_str())
        else {
            continue;
        };
        let Some(cleaned) = clean_voice_item(raw) else {
            continue;
        };
        if seen.insert(cleaned.to_lowercase()) {
            names.push(cleaned);
        }
    }
    names
}

fn clean_voice_item(raw: &str) -> Option<String> {
    let cleaned = strip(&TRAILING_PAREN, raw.trim());
    let cleaned = strip(&TRAILING_ANNOTATION, &cleaned);
    let cleaned = cleaned.trim();
    if omni_core::js::utf16_len(cleaned) < 2 || cleaned.starts_with('<') {
        return None;
    }
    Some(cleaned.to_owned())
}
