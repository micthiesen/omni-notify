//! `html_to_text` against html-to-text 10 output with the production options
//! (committed `tests/golden/html_to_text.json`).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_email::html_to_text::html_to_text;
use serde::Deserialize;

#[derive(Deserialize)]
struct Case {
    html: String,
    text: String,
}

#[test]
fn matches_the_node_library_output() {
    let raw = include_str!("golden/html_to_text.json");
    let cases: Vec<Case> = serde_json::from_str(raw).unwrap();
    let failures: Vec<String> = cases
        .iter()
        .filter_map(|case| {
            let actual = html_to_text(&case.html);
            (actual != case.text).then(|| {
                format!(
                    "{:?}\n  expected {:?}\n  actual   {:?}",
                    case.html, case.text, actual
                )
            })
        })
        .collect();
    assert!(
        failures.is_empty(),
        "{} mismatches:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
