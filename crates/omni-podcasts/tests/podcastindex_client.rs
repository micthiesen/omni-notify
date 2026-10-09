//! Port of `src/podcast-recs/podcastindex/client.spec.ts` (named
//! `podcastindex_client`: the Castro client spec shares the `client` stem),
//! plus the bounded search request against a local mock.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_podcasts::podcastindex::{
    PodcastIndexEpisode, RawPodcastIndexEpisode, map_episode, parse_search_by_person,
};

fn raw_episode() -> RawPodcastIndexEpisode {
    RawPodcastIndexEpisode {
        title: Some("Episode Title".into()),
        feed_title: Some("Show Title".into()),
        feed_url: Some("https://example.com/feed.xml".into()),
        feed_itunes_id: Some(12345.0),
        guid: Some("episode-guid".into()),
        enclosure_url: Some("https://example.com/episode.mp3".into()),
        link: Some("https://example.com/episode".into()),
        date_published: Some(1_700_000_000.0),
        duration: Some(1_800.0),
        description: Some("Episode description".into()),
        image: Some("https://example.com/episode.png".into()),
        feed_image: Some("https://example.com/feed.png".into()),
    }
}

#[test]
fn maps_a_realistic_raw_episode() {
    assert_eq!(
        map_episode(&raw_episode()),
        Some(PodcastIndexEpisode {
            title: "Episode Title".into(),
            feed_title: "Show Title".into(),
            feed_url: "https://example.com/feed.xml".into(),
            feed_itunes_id: Some(12345),
            guid: "episode-guid".into(),
            enclosure_url: "https://example.com/episode.mp3".into(),
            episode_url: Some("https://example.com/episode".into()),
            published_at: 1_700_000_000_000,
            duration_minutes: Some(30),
            description: "Episode description".into(),
            artwork_url: Some("https://example.com/episode.png".into()),
        })
    );
}

#[test]
fn multiplies_date_published_seconds_by_1000_for_published_at_ms() {
    let raw = RawPodcastIndexEpisode {
        date_published: Some(1_600_000_000.0),
        ..raw_episode()
    };
    assert_eq!(map_episode(&raw).unwrap().published_at, 1_600_000_000_000);
}

#[test]
fn rounds_duration_minutes_from_duration_in_seconds() {
    let raw = RawPodcastIndexEpisode {
        duration: Some(125.0),
        ..raw_episode()
    };
    assert_eq!(map_episode(&raw).unwrap().duration_minutes, Some(2));
}

#[test]
fn omits_duration_minutes_when_duration_is_absent_or_zero() {
    for duration in [None, Some(0.0)] {
        let raw = RawPodcastIndexEpisode {
            duration,
            ..raw_episode()
        };
        assert_eq!(map_episode(&raw).unwrap().duration_minutes, None);
    }
}

#[test]
fn falls_back_artwork_url_from_image_to_feed_image() {
    let raw = RawPodcastIndexEpisode {
        image: None,
        ..raw_episode()
    };
    assert_eq!(
        map_episode(&raw).unwrap().artwork_url.as_deref(),
        Some("https://example.com/feed.png")
    );
}

#[test]
fn omits_artwork_url_when_neither_image_nor_feed_image_is_set() {
    let raw = RawPodcastIndexEpisode {
        image: None,
        feed_image: None,
        ..raw_episode()
    };
    assert_eq!(map_episode(&raw).unwrap().artwork_url, None);
}

#[test]
fn omits_feed_itunes_id_when_0() {
    let raw = RawPodcastIndexEpisode {
        feed_itunes_id: Some(0.0),
        ..raw_episode()
    };
    assert_eq!(map_episode(&raw).unwrap().feed_itunes_id, None);
}

#[test]
fn omits_feed_itunes_id_when_absent() {
    let raw = RawPodcastIndexEpisode {
        feed_itunes_id: None,
        ..raw_episode()
    };
    assert_eq!(map_episode(&raw).unwrap().feed_itunes_id, None);
}

#[test]
fn sets_episode_url_from_link() {
    let raw = RawPodcastIndexEpisode {
        link: Some("https://example.com/ep/1".into()),
        ..raw_episode()
    };
    assert_eq!(
        map_episode(&raw).unwrap().episode_url.as_deref(),
        Some("https://example.com/ep/1")
    );
}

#[test]
fn returns_undefined_when_enclosure_url_is_missing() {
    let raw = RawPodcastIndexEpisode {
        enclosure_url: None,
        ..raw_episode()
    };
    assert_eq!(map_episode(&raw), None);
}

#[test]
fn returns_undefined_when_feed_url_is_missing() {
    let raw = RawPodcastIndexEpisode {
        feed_url: None,
        ..raw_episode()
    };
    assert_eq!(map_episode(&raw), None);
}

#[test]
fn returns_undefined_when_date_published_is_missing() {
    let raw = RawPodcastIndexEpisode {
        date_published: None,
        ..raw_episode()
    };
    assert_eq!(map_episode(&raw), None);
}

#[test]
fn returns_undefined_when_guid_is_missing_episode_identity() {
    let raw = RawPodcastIndexEpisode {
        guid: None,
        ..raw_episode()
    };
    assert_eq!(map_episode(&raw), None);
}

#[test]
fn decodes_explicit_nulls_and_null_items() {
    let body = r#"{"items":[{"title":"T","feedTitle":null,"feedUrl":"https://f","feedItunesId":null,
        "guid":"g","enclosureUrl":"https://e.mp3","link":null,"datePublished":1700000000,
        "duration":null,"description":null,"image":null,"feedImage":null,"extra":1},
        {"title":"skip me","guid":null}]}"#;
    let episodes = parse_search_by_person("Someone", body).unwrap();
    assert_eq!(episodes.len(), 1);
    assert_eq!(episodes[0].feed_itunes_id, None);
    assert!(
        parse_search_by_person("Someone", r#"{"items":null}"#)
            .unwrap()
            .is_empty()
    );
    assert!(parse_search_by_person("Someone", "{}").unwrap().is_empty());
}
