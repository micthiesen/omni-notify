//! Streamer display helpers shared by Home, Live, the rail and the palette.

use leptos::prelude::*;
use omni_api::intelligence::LivestreamIntelligence;
use omni_api::streamers::{DggPresence, LiveStreamerView, StreamerBinding, StreamerView};

use super::badges::Tag;
use super::tone::Tone;
use crate::utils::format::format_compact_number;
use omni_api::common::encode_uri_component;

pub fn streamer_path(id: &str) -> String {
    format!("/streamers/{}", encode_uri_component(id))
}

pub fn platform_label(platform: &str) -> String {
    if platform.eq_ignore_ascii_case("youtube") {
        return "YouTube".to_owned();
    }
    let mut chars = platform.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
        None => String::new(),
    }
}

/// Watch order among live bindings: YouTube, then Kick, then Twitch.
fn watch_rank(platform: &str) -> u8 {
    match platform.to_ascii_lowercase().as_str() {
        "youtube" => 0,
        "kick" => 1,
        "twitch" => 2,
        _ => 3,
    }
}

/// The binding to open when watching a live streamer: the most preferred
/// platform (YouTube, Kick, Twitch) among bindings with a live source. A
/// streamer with no matching live source falls back to its primary binding.
pub fn preferred_watch(streamer: &LiveStreamerView) -> StreamerBinding {
    streamer
        .bindings
        .iter()
        .filter(|b| {
            streamer.sources.iter().any(|s| {
                s.platform.eq_ignore_ascii_case(&b.platform)
                    && s.username.eq_ignore_ascii_case(&b.username)
            })
        })
        .min_by_key(|b| watch_rank(&b.platform))
        .cloned()
        .unwrap_or_else(|| streamer.primary.clone())
}

/// The typed intelligence document of a live streamer, when present and valid.
pub fn streamer_intelligence(streamer: &LiveStreamerView) -> Option<LivestreamIntelligence> {
    serde_json::from_value(streamer.intelligence.clone()?).ok()
}

/// Current viewers when known, else this session's peak.
pub fn viewers_label(streamer: &LiveStreamerView) -> Option<String> {
    let known: Vec<_> = streamer
        .sources
        .iter()
        .filter(|s| s.viewer_count.is_some())
        .collect();
    if known.len() > 1 {
        return Some(
            known
                .iter()
                .map(|source| {
                    format!(
                        "{} {}",
                        format_compact_number(source.viewer_count.unwrap_or(0) as f64),
                        platform_label(&source.platform)
                    )
                })
                .collect::<Vec<_>>()
                .join(" + "),
        );
    }
    if let Some(count) = streamer.viewer_count {
        return Some(format!("{} watching", format_compact_number(count as f64)));
    }
    (streamer.max_viewer_count > 0).then(|| {
        format!(
            "{} peak",
            format_compact_number(streamer.max_viewer_count as f64)
        )
    })
}

/// Live streamers in snapshot order.
pub fn live_streamers(streamers: &[StreamerView]) -> Vec<LiveStreamerView> {
    streamers
        .iter()
        .filter_map(|s| match s {
            StreamerView::Live(live) => Some(live.clone()),
            StreamerView::Offline(_) => None,
        })
        .collect()
}

/// Summed current viewers (falls back to the session peak).
pub fn viewer_number(streamer: &LiveStreamerView) -> i64 {
    streamer.viewer_count.unwrap_or(streamer.max_viewer_count)
}

/// "Hosted on DGG", "<n> on DGG" or "DGG".
#[component]
pub fn DggPresenceTag(dgg: Option<DggPresence>) -> impl IntoView {
    dgg.map(|dgg| {
        let label = if dgg.hosted {
            "Hosted on DGG".to_owned()
        } else if let Some(viewers) = dgg.viewers {
            format!("{} on DGG", format_compact_number(viewers as f64))
        } else {
            "DGG".to_owned()
        };
        let tone = if dgg.hosted {
            Tone::Info
        } else {
            Tone::Neutral
        };
        view! { <Tag tone>{label}</Tag> }
    })
}

#[cfg(test)]
mod tests {
    use omni_api::streamers::{LiveSourceView, LiveTrue, StreamerTier};

    use super::*;

    fn binding(platform: &str, username: &str) -> StreamerBinding {
        StreamerBinding {
            platform: platform.into(),
            username: username.into(),
            url: format!("https://{platform}.example/{username}"),
        }
    }

    fn source(platform: &str, username: &str) -> LiveSourceView {
        LiveSourceView {
            platform: platform.into(),
            username: username.into(),
            title: String::new(),
            viewer_count: None,
            category: None,
        }
    }

    fn streamer(live: &[(&str, &str)], primary: StreamerBinding) -> LiveStreamerView {
        LiveStreamerView {
            id: "destiny".into(),
            display_name: "Destiny".into(),
            bindings: vec![
                binding("twitch", "destiny"),
                binding("kick", "destiny"),
                binding("youtube", "@destiny"),
            ],
            tier: StreamerTier::Primary,
            dgg: None,
            live: LiveTrue,
            title: String::new(),
            started_at: 0,
            max_viewer_count: 0,
            viewer_count: None,
            sources: live.iter().map(|(p, u)| source(p, u)).collect(),
            category: None,
            primary,
            intelligence: None,
        }
    }

    #[test]
    fn youtube_wins_over_kick_and_twitch() {
        let s = streamer(
            &[
                ("twitch", "destiny"),
                ("kick", "destiny"),
                ("youtube", "@destiny"),
            ],
            binding("twitch", "destiny"),
        );
        assert_eq!(preferred_watch(&s).platform, "youtube");
    }

    #[test]
    fn kick_wins_over_twitch_when_youtube_is_offline() {
        let s = streamer(
            &[("twitch", "destiny"), ("kick", "destiny")],
            binding("twitch", "destiny"),
        );
        assert_eq!(preferred_watch(&s).platform, "kick");
    }

    #[test]
    fn a_single_live_binding_is_used_even_when_less_preferred() {
        let s = streamer(&[("twitch", "Destiny")], binding("twitch", "destiny"));
        let watch = preferred_watch(&s);
        assert_eq!(watch.platform, "twitch");
        assert_eq!(watch.url, "https://twitch.example/destiny");
    }

    #[test]
    fn offline_bindings_are_never_chosen() {
        // Only Kick is live: the configured YouTube binding is not on air.
        let s = streamer(&[("kick", "destiny")], binding("kick", "destiny"));
        assert_eq!(preferred_watch(&s).platform, "kick");
    }

    #[test]
    fn falls_back_to_the_primary_without_matching_sources() {
        let s = streamer(&[], binding("kick", "destiny"));
        assert_eq!(preferred_watch(&s).platform, "kick");
    }

    #[test]
    fn youtube_is_labelled_with_its_brand_casing() {
        assert_eq!(platform_label("youtube"), "YouTube");
        assert_eq!(platform_label("kick"), "Kick");
    }
}
