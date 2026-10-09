//! The `PodcastRecs` task, plus manual-input parsing.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_podcasts::task::parse_max_recommendations;
use omni_podcasts::taste::describe_unreadable_file;
use serde_json::json;

#[test]
fn accepts_a_readable_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("taste.md");
    std::fs::write(&file, "profile").unwrap();
    assert_eq!(describe_unreadable_file(file.to_str().unwrap()), None);
}

#[test]
fn rejects_a_directory_the_eisdir_footgun() {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        describe_unreadable_file(dir.path().to_str().unwrap())
            .unwrap()
            .contains("not a file")
    );
}

#[test]
fn rejects_a_missing_path() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("nope.md");
    assert!(
        describe_unreadable_file(missing.to_str().unwrap())
            .unwrap()
            .contains("could not be read")
    );
}

#[test]
fn parses_manual_run_input() {
    assert_eq!(
        parse_max_recommendations(&json!({ "maxRecommendations": 3 })),
        Ok(3)
    );
    assert_eq!(
        parse_max_recommendations(&json!({ "maxRecommendations": 5.0 })),
        Ok(5)
    );
    for bad in [
        json!({ "maxRecommendations": 0 }),
        json!({ "maxRecommendations": 6 }),
        json!({ "maxRecommendations": 2.5 }),
        json!({ "maxRecommendations": "3" }),
        json!(null),
    ] {
        assert_eq!(
            parse_max_recommendations(&bad),
            Err("maxRecommendations must be an integer from 1 to 5".to_owned())
        );
    }
}
