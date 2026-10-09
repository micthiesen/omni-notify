//! Podcast RSS reading.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use jiff::tz::TimeZone;
use omni_podcasts::rss::{
    FeedEpisode, find_episode_by_title, parse_feed_episodes, parse_feed_episodes_default,
};

const FIXTURE_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:itunes="http://www.itunes.com/dtds/podcast-1.0.dtd">
<channel>
<title>Test Podcast</title>
<item>
  <title><![CDATA[Episode One: Tom &amp; Jerry's "Big" Day]]></title>
  <guid isPermaLink="false">guid-episode-one</guid>
  <pubDate>Thu, 02 Jan 2025 12:00:00 GMT</pubDate>
  <itunes:duration>1:02:03</itunes:duration>
  <description><![CDATA[<p>An episode about &amp; things.</p><p>Extra   spaces   here.</p>]]></description>
  <link>https://example.com/episodes/one</link>
</item>
<item>
  <title>Episode Two: Seconds Duration</title>
  <guid isPermaLink="false">guid-episode-two</guid>
  <pubDate>Fri, 03 Jan 2025 12:00:00 GMT</pubDate>
  <itunes:duration>3720</itunes:duration>
  <itunes:summary>Summary only, no description tag.</itunes:summary>
</item>
<item>
  <title>Episode Three: No Guid, Has Enclosure</title>
  <enclosure url="https://example.com/audio/three.mp3" length="123" type="audio/mpeg"/>
  <pubDate>Sat, 04 Jan 2025 12:00:00 GMT</pubDate>
</item>
<item>
  <title>Episode Four: Invalid Date Is Skipped</title>
  <guid>guid-episode-four</guid>
  <pubDate>not-a-real-date</pubDate>
</item>
<item>
  <title>Episode Five: No Duration Tag</title>
  <guid>guid-episode-five</guid>
  <pubDate>Sun, 05 Jan 2025 12:00:00 GMT</pubDate>
</item>
</channel>
</rss>"#;

fn episodes() -> Vec<FeedEpisode> {
    parse_feed_episodes_default(FIXTURE_XML, &TimeZone::UTC)
}

fn by_guid(guid: &str) -> FeedEpisode {
    episodes().into_iter().find(|e| e.guid == guid).unwrap()
}

#[test]
fn skips_items_with_an_unparseable_pub_date() {
    assert!(!episodes().iter().any(|e| e.title.contains("Invalid Date")));
}

#[test]
fn parses_the_expected_number_of_valid_episodes() {
    assert_eq!(episodes().len(), 4);
}

#[test]
fn decodes_cdata_wrapped_entity_encoded_titles() {
    assert_eq!(
        by_guid("guid-episode-one").title,
        r#"Episode One: Tom & Jerry's "Big" Day"#
    );
}

#[test]
fn parses_itunes_duration_in_hh_mm_ss_form() {
    assert_eq!(by_guid("guid-episode-one").duration_minutes, Some(62));
}

#[test]
fn parses_itunes_duration_given_as_plain_seconds() {
    assert_eq!(by_guid("guid-episode-two").duration_minutes, Some(62));
}

#[test]
fn strips_html_tags_decodes_entities_and_collapses_whitespace_in_the_description() {
    assert_eq!(
        by_guid("guid-episode-one").description,
        "An episode about & things. Extra spaces here."
    );
}

#[test]
fn falls_back_to_itunes_summary_when_description_is_missing() {
    assert_eq!(
        by_guid("guid-episode-two").description,
        "Summary only, no description tag."
    );
}

#[test]
fn falls_back_to_the_enclosure_url_when_guid_is_missing() {
    let episode = by_guid("https://example.com/audio/three.mp3");
    assert_eq!(episode.title, "Episode Three: No Guid, Has Enclosure");
}

#[test]
fn omits_duration_minutes_when_itunes_duration_is_absent() {
    assert_eq!(by_guid("guid-episode-five").duration_minutes, None);
}

#[test]
fn captures_the_link_element_when_present() {
    assert_eq!(
        by_guid("guid-episode-one").link.as_deref(),
        Some("https://example.com/episodes/one")
    );
}

#[test]
fn sorts_episodes_newest_first() {
    let dates: Vec<i64> = episodes().iter().map(|e| e.published_at).collect();
    let mut sorted = dates.clone();
    sorted.sort_by(|a, b| b.cmp(a));
    assert_eq!(dates, sorted);
}

#[test]
fn caps_results_at_max_episodes() {
    assert_eq!(parse_feed_episodes(FIXTURE_XML, 2, &TimeZone::UTC).len(), 2);
}

#[test]
fn finds_an_exact_normalized_match() {
    let episodes = episodes();
    let found = find_episode_by_title(&episodes, "episode two seconds duration");
    assert_eq!(found.map(|e| e.guid.as_str()), Some("guid-episode-two"));
}

#[test]
fn finds_the_longest_containment_match_when_the_query_is_a_substring() {
    let episodes = episodes();
    let found = find_episode_by_title(&episodes, "Seconds Duration");
    assert_eq!(found.map(|e| e.guid.as_str()), Some("guid-episode-two"));
}

#[test]
fn finds_a_match_when_the_query_is_longer_than_the_title() {
    let episodes = episodes();
    let found = find_episode_by_title(&episodes, "Episode Two: Seconds Duration (Director's Cut)");
    assert_eq!(found.map(|e| e.guid.as_str()), Some("guid-episode-two"));
}

#[test]
fn returns_undefined_when_nothing_matches() {
    let episodes = episodes();
    assert!(find_episode_by_title(&episodes, "Totally Unrelated Title").is_none());
}

#[test]
fn returns_undefined_for_an_empty_episode_list() {
    assert!(find_episode_by_title(&[], "Anything").is_none());
}

#[test]
fn tolerates_malformed_markup_and_html_entities() {
    let xml = r#"<rss><channel><item><title>A &amp;amp; B &nbsp;&#8217;s</title><guid>g</guid>
        <pubDate>Thu, 02 Jan 2025 12:00:00 GMT</pubDate><description><p>Unclosed <b>bold</description>
        </item><item><title>Truncated"#;
    let episodes = parse_feed_episodes_default(xml, &TimeZone::UTC);
    assert_eq!(episodes.len(), 1);
    assert_eq!(episodes[0].title, "A & B \u{a0}\u{2019}s");
    assert_eq!(episodes[0].description, "Unclosed bold");
}

#[test]
fn keeps_every_item_when_a_feed_has_stray_ampersands_or_angle_brackets() {
    // Bare `&` and `<` read as text; one must not lose the rest of the feed.
    let xml = r#"<rss><channel>
<item><title>First AT&T Story</title><guid>g1</guid><pubDate>Tue, 01 Jul 2025 10:00:00 GMT</pubDate>
<enclosure url="https://cdn.example.com/a.mp3?x=1&y=2" /></item>
<item><title>Second</title><guid>g2</guid><pubDate>Mon, 30 Jun 2025 10:00:00 GMT</pubDate>
<description>a < b and 5 && 6</description></item>
<item><title><![CDATA[Third & <b>raw</b>]]></title><guid>g3</guid><pubDate>Sun, 29 Jun 2025 10:00:00 GMT</pubDate></item>
</channel></rss>"#;
    let episodes = parse_feed_episodes_default(xml, &TimeZone::UTC);
    assert_eq!(episodes.len(), 3);
    assert_eq!(episodes[0].title, "First AT&T Story");
    assert_eq!(
        episodes[0].enclosure_url.as_deref(),
        Some("https://cdn.example.com/a.mp3?x=1&y=2")
    );
    assert_eq!(episodes[1].description, "a < b and 5 && 6");
    // CDATA is copied verbatim, not escaped.
    assert_eq!(episodes[2].title, "Third & <b>raw</b>");
}
