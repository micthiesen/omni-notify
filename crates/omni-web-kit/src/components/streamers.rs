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

/// A rolling summary older than this no longer describes the stream: windows
/// land about every eight minutes, so this allows one missed window.
const FRESH_READ_MS: f64 = 20.0 * 60_000.0;

/// What a live stream is about right now, from its latest rolling summary.
#[derive(Clone, Debug, PartialEq)]
pub struct LiveRead {
    pub topic: String,
    pub summary: String,
}

/// The latest rolling summary, when it is fresh at `now_ms`.
pub fn live_read(intelligence: &LivestreamIntelligence, now_ms: f64) -> Option<LiveRead> {
    let summary = intelligence.summary.as_ref()?;
    #[allow(clippy::cast_precision_loss)]
    let age = now_ms - summary.updated_at as f64;
    (age <= FRESH_READ_MS && !summary.topic.trim().is_empty()).then(|| LiveRead {
        topic: summary.topic.clone(),
        summary: summary.text.clone(),
    })
}

fn is_decoration(c: char) -> bool {
    matches!(u32::from(c),
        0x1F000..=0x1FAFF   // emoji, pictographs, symbols
        | 0x2600..=0x27BF   // miscellaneous symbols and dingbats
        | 0x2B00..=0x2BFF   // arrows, stars
        | 0x2190..=0x21FF   // arrows
        | 0xFE00..=0xFE0F   // variation selectors
        | 0x200D            // zero-width joiner
        | 0x20E3            // keycap
        | 0xE0000..=0xE007F // tags
    )
}

/// A stream title without emoji and decorations, with whitespace collapsed:
/// "🚨ALL IN PODCAST HATEWATCH🚨" reads "ALL IN PODCAST HATEWATCH". A title
/// made only of decorations is returned unchanged.
pub fn clean_stream_title(title: &str) -> String {
    let cleaned = title
        .chars()
        .map(|c| if is_decoration(c) { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if cleaned.is_empty() {
        title.trim().to_owned()
    } else {
        cleaned
    }
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
            avatar_url: None,
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

    #[test]
    fn stream_titles_lose_emoji_and_extra_whitespace() {
        assert_eq!(
            clean_stream_title("\u{1f6a8}ALL IN PODCAST HATEWATCH\u{1f6a8}"),
            "ALL IN PODCAST HATEWATCH"
        );
        assert_eq!(
            clean_stream_title("nopixel V \u{1f453}LIVE\u{1f453}BIG DAY  \u{2764}\u{fe0f}"),
            "nopixel V LIVE BIG DAY"
        );
        assert_eq!(
            clean_stream_title("lizards, gimps and pilots"),
            "lizards, gimps and pilots"
        );
        assert_eq!(
            clean_stream_title(" \u{1f525}\u{1f525} "),
            "\u{1f525}\u{1f525}"
        );
    }

    fn intelligence(summary: Option<(&str, i64)>) -> LivestreamIntelligence {
        serde_json::from_value(serde_json::json!({
            "streamerId": "destiny",
            "sessionStartedAt": 0,
            "relevanceScore": 0,
            "relevanceReasons": [],
            "chapters": [],
            "updatedAt": 0,
            "summary": summary.map(|(topic, at)| serde_json::json!({
                "text": "Destiny argues about the claim.",
                "topic": topic,
                "confidence": 0.9,
                "transcriptExcerpt": "",
                "updatedAt": at,
                "windowSeconds": 480,
            })),
        }))
        .expect("intelligence")
    }

    #[test]
    fn a_live_read_is_the_latest_summary_while_it_is_fresh() {
        let minute = 60_000.0;
        let read = live_read(
            &intelligence(Some(("Debate over the claim", 0))),
            19.0 * minute,
        );
        assert_eq!(
            read,
            Some(LiveRead {
                topic: "Debate over the claim".into(),
                summary: "Destiny argues about the claim.".into(),
            })
        );
        assert_eq!(
            live_read(&intelligence(Some(("Debate", 0))), 21.0 * minute),
            None
        );
        assert_eq!(live_read(&intelligence(Some((" ", 0))), 0.0), None);
        assert_eq!(live_read(&intelligence(None), 0.0), None);
    }
}
