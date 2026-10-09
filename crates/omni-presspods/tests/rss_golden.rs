//! The feed must match the TypeScript `podcast`-package output byte for byte
//! (after normalizing `lastBuildDate`). `tests/golden/rss.xml` is generated
//! from the shipped TS builder by `scripts/golden-rss.ts` over
//! `tests/golden/rss-episodes.json`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_presspods::model::PressPodsEpisode;
use omni_presspods::rss::{FEED_EPISODE_LIMIT, build_feed, feed_etag};

fn golden_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

fn episodes() -> Vec<PressPodsEpisode> {
    let raw = std::fs::read_to_string(golden_dir().join("rss-episodes.json")).unwrap();
    serde_json::from_str(&raw).unwrap()
}

fn normalize(xml: &str) -> String {
    let start = xml.find("<lastBuildDate>").unwrap() + "<lastBuildDate>".len();
    let end = xml[start..].find("</lastBuildDate>").unwrap() + start;
    format!("{}LAST_BUILD_DATE{}", &xml[..start], &xml[end..])
}

#[test]
fn feed_matches_the_typescript_golden_byte_for_byte() {
    let golden = std::fs::read_to_string(golden_dir().join("rss.xml")).unwrap();
    let rust = build_feed("https://pods.example.test", &episodes(), 1_791_547_200_000);
    assert_eq!(normalize(&rust), golden);
    assert!(rust.contains("<lastBuildDate>Fri, 09 Oct 2026 12:00:00 GMT</lastBuildDate>"));
}

#[test]
fn feed_lists_at_most_fifty_episodes() {
    let template = episodes().remove(1);
    let many: Vec<PressPodsEpisode> = (0..FEED_EPISODE_LIMIT + 3)
        .map(|i| PressPodsEpisode {
            episode_id: format!("ep{i}"),
            ..template.clone()
        })
        .collect();
    let xml = build_feed("https://pods.example.test", &many, 0);
    assert_eq!(xml.matches("<item>").count(), FEED_EPISODE_LIMIT);
    assert!(xml.contains(">ep0<") && !xml.contains(">ep50<"));
}

#[test]
fn etag_is_the_newest_episode_id() {
    assert_eq!(feed_etag(&episodes()), "\"ZmVlZC1nb2xkZW4tZXBpc29kZS0x\"");
    assert_eq!(feed_etag(&[]), "\"no-episodes\"");
}
