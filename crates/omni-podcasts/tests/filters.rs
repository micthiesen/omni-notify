//! Port of `src/podcast-recs/filters.spec.ts`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashSet;

use omni_podcasts::filters::{
    PodcastFilterContext, RECENT_EPISODE_WINDOW_MS, filter_eligible_episodes,
};
use omni_podcasts::persistence::PodcastExclusions;
use omni_podcasts::titles::normalize_title;
use omni_podcasts::types::EpisodeCandidate;

const NOW: i64 = 1_784_160_000_000; // Date.UTC(2026, 6, 16)
const DAY: i64 = 24 * 60 * 60 * 1000;

fn candidate() -> EpisodeCandidate {
    EpisodeCandidate {
        episode_id: "itunes:1#guid-1".into(),
        show_id: "itunes:1".into(),
        show_title: "The Gray Area".into(),
        episode_title: "What is consciousness?".into(),
        feed_url: "https://feeds.example.com/grayarea".into(),
        itunes_id: Some(1),
        episode_guid: "guid-1".into(),
        published_at: NOW - 2 * DAY,
        description: "A conversation about minds.".into(),
        show_genres: vec!["Philosophy".into()],
        discovered_via: "reddit thread".into(),
        ..EpisodeCandidate::default()
    }
}

struct Ctx {
    ids: HashSet<String>,
    titles: HashSet<String>,
    exclusions: PodcastExclusions,
}

impl Ctx {
    fn new() -> Self {
        Self {
            ids: HashSet::new(),
            titles: HashSet::new(),
            exclusions: PodcastExclusions::default(),
        }
    }

    fn context(&self) -> PodcastFilterContext<'_> {
        PodcastFilterContext {
            now: NOW,
            subscribed_show_ids: &self.ids,
            subscribed_show_titles: &self.titles,
            exclusions: &self.exclusions,
        }
    }
}

fn first_reason(ctx: &Ctx, c: EpisodeCandidate) -> String {
    filter_eligible_episodes(vec![c], &ctx.context()).dropped[0]
        .reason
        .clone()
}

#[test]
fn keeps_a_fresh_unexcluded_episode() {
    let ctx = Ctx::new();
    let result = filter_eligible_episodes(vec![candidate()], &ctx.context());
    assert_eq!(result.kept.len(), 1);
    assert!(result.dropped.is_empty());
}

#[test]
fn drops_episodes_outside_the_recency_window() {
    let ctx = Ctx::new();
    let stale = EpisodeCandidate {
        published_at: NOW - RECENT_EPISODE_WINDOW_MS - DAY,
        ..candidate()
    };
    let result = filter_eligible_episodes(vec![stale], &ctx.context());
    assert!(result.kept.is_empty());
    assert!(result.dropped[0].reason.contains("outside recency window"));
}

#[test]
fn drops_episodes_with_a_future_release_date() {
    let ctx = Ctx::new();
    let future = EpisodeCandidate {
        published_at: NOW + 2 * DAY,
        ..candidate()
    };
    assert!(first_reason(&ctx, future).contains("future"));
}

#[test]
fn drops_already_recommended_episodes() {
    let mut ctx = Ctx::new();
    ctx.exclusions.episode_ids.insert("itunes:1#guid-1".into());
    assert_eq!(
        first_reason(&ctx, candidate()),
        "episode already recommended"
    );
}

#[test]
fn drops_shows_on_cooldown_or_excluded_by_feedback() {
    let mut ctx = Ctx::new();
    ctx.exclusions.show_ids.insert("itunes:1".into());
    assert_eq!(
        first_reason(&ctx, candidate()),
        "show on cooldown or excluded by feedback"
    );
}

#[test]
fn drops_subscribed_shows_by_canonical_id() {
    let mut ctx = Ctx::new();
    ctx.ids.insert("itunes:1".into());
    assert_eq!(first_reason(&ctx, candidate()), "already subscribed");
}

#[test]
fn drops_subscribed_shows_by_normalized_title_when_ids_are_missing() {
    let mut ctx = Ctx::new();
    ctx.titles.insert(normalize_title("The Gray Area"));
    assert_eq!(
        first_reason(&ctx, candidate()),
        "already subscribed (title match)"
    );
}

#[test]
fn ignores_case_punctuation_and_diacritics() {
    assert_eq!(
        normalize_title("Séan Carroll's Mindscape!"),
        normalize_title("sean carrolls mindscape")
    );
}

#[test]
fn collapses_whitespace() {
    assert_eq!(
        normalize_title("The  Rest   Is History"),
        "the rest is history"
    );
}
