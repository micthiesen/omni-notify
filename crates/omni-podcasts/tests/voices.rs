//! Port of `src/podcast-recs/voices.spec.ts`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_podcasts::voices::parse_voices;

const FIXTURE: &str = "# Podcast Taste Profile

## Favorite genres

- True crime
- Tech interviews

## Voices I follow — recommend their guest spots anywhere

- Jesse Singal (Blocked and Reported)
- Ezra Klein
- <add more>
- jesse singal

## Shows to avoid

- Some Show I Hate
- Another Bad Show
";

#[test]
fn collects_names_from_the_voices_section_only() {
    assert_eq!(parse_voices(FIXTURE), vec!["Jesse Singal", "Ezra Klein"]);
}

#[test]
fn does_not_pick_up_bullets_from_other_sections() {
    let names = parse_voices(FIXTURE);
    assert!(!names.contains(&"True crime".to_owned()));
    assert!(!names.contains(&"Some Show I Hate".to_owned()));
}

#[test]
fn strips_a_trailing_parenthetical() {
    assert!(parse_voices(FIXTURE).contains(&"Jesse Singal".to_owned()));
}

#[test]
fn dedupes_case_insensitively_keeping_the_first_occurrences_casing() {
    let names = parse_voices(FIXTURE);
    let matching: Vec<_> = names
        .iter()
        .filter(|n| n.to_lowercase() == "jesse singal")
        .collect();
    assert_eq!(matching, vec!["Jesse Singal"]);
}

#[test]
fn drops_placeholder_items() {
    assert!(!parse_voices(FIXTURE).contains(&"<add more>".to_owned()));
}

#[test]
fn returns_empty_when_there_is_no_voices_section() {
    let no_voices = "# Podcast Taste Profile\n\n## Favorite genres\n\n- True crime\n";
    assert!(parse_voices(no_voices).is_empty());
}

#[test]
fn returns_empty_for_empty_input() {
    assert!(parse_voices("").is_empty());
}

#[test]
fn preserves_hyphenated_apostrophe_names_and_ignores_h3_subheaders() {
    let md = [
        "## Voices I follow — recommend their guest spots anywhere",
        "### Core",
        "- Jean-Luc Picard",
        "- Anne-Marie Slaughter",
        "### More",
        "- Alex O'Connor",
        "",
        "## Taste",
        "- not a voice",
    ]
    .join("\n");
    assert_eq!(
        parse_voices(&md),
        vec!["Jean-Luc Picard", "Anne-Marie Slaughter", "Alex O'Connor"]
    );
}
