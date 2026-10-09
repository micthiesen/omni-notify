use regex::RegexBuilder;

/// The text a model wrapped in `<tag>…</tag>`.
///
/// Autoregressive models occasionally truncate before the closing tag, change
/// its case, or start a partial `</tag` fragment. A well-formed pair wins;
/// otherwise everything after an unclosed opening tag is taken, minus any
/// dangling close fragment. `None` when nothing usable is found.
pub fn extract_between_tags(text: &str, tag: &str) -> Option<String> {
    let tag = regex::escape(tag);
    let build = |pattern: String| {
        RegexBuilder::new(&pattern)
            .case_insensitive(true)
            .dot_matches_new_line(true)
            .build()
            .ok()
    };
    if let Some(paired) = build(format!("<{tag}>(.*?)</{tag}>"))
        .and_then(|re| re.captures(text).and_then(|c| c.get(1)))
        .map(|m| m.as_str().trim().to_owned())
        .filter(|s| !s.is_empty())
    {
        return Some(paired);
    }
    let open = build(format!("<{tag}>(.*)$"))?;
    let tail = open.captures(text)?.get(1)?.as_str();
    let dangling = build(format!(r"</?{tag}[^>]*>?\s*$"))?;
    let tail = dangling.replace(tail, "").trim().to_owned();
    (!tail.is_empty()).then_some(tail)
}

/// The error message when no content was found between the tags.
pub fn missing_tags_message(tag: &str) -> String {
    format!("Failed to extract content between <{tag}> tags")
}

#[cfg(test)]
mod parsing_spec {
    //! Tag extraction cases.
    use super::extract_between_tags;

    #[test]
    fn extracts_a_well_formed_tag_pair() {
        assert_eq!(
            extract_between_tags(
                "<cleaned_article>hello</cleaned_article>",
                "cleaned_article"
            )
            .as_deref(),
            Some("hello")
        );
    }

    #[test]
    fn trims_surrounding_whitespace() {
        assert_eq!(
            extract_between_tags("<x>\n  body  \n</x>", "x").as_deref(),
            Some("body")
        );
    }

    #[test]
    fn ignores_preamble_trailing_text_outside_the_tags() {
        assert_eq!(
            extract_between_tags("here you go:\n<x>body</x>\nthanks", "x").as_deref(),
            Some("body")
        );
    }

    #[test]
    fn matches_tags_case_insensitively() {
        assert_eq!(
            extract_between_tags("<X>body</X>", "x").as_deref(),
            Some("body")
        );
    }

    #[test]
    fn recovers_when_the_closing_tag_is_missing() {
        assert_eq!(
            extract_between_tags("<x>the whole body was cut off", "x").as_deref(),
            Some("the whole body was cut off")
        );
    }

    #[test]
    fn recovers_when_the_closing_tag_lost_its_bracket() {
        assert_eq!(
            extract_between_tags("<x>body text</x", "x").as_deref(),
            Some("body text")
        );
    }

    #[test]
    fn throws_when_the_opening_tag_is_absent_entirely() {
        assert_eq!(extract_between_tags("no tags at all", "x"), None);
        assert_eq!(
            super::missing_tags_message("x"),
            "Failed to extract content between <x> tags"
        );
    }

    #[test]
    fn throws_when_the_body_is_empty() {
        assert_eq!(extract_between_tags("<x></x>", "x"), None);
    }
}
