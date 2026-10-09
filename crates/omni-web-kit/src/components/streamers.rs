//! Streamer display helpers shared by Home, Live, the rail and the palette.

use leptos::prelude::*;
use omni_api::intelligence::LivestreamIntelligence;
use omni_api::streamers::{DggPresence, LiveStreamerView, StreamerView};

use super::badges::Tag;
use super::tone::Tone;
use crate::utils::format::format_compact_number;
use omni_api::common::encode_uri_component;

pub fn streamer_path(id: &str) -> String {
    format!("/streamers/{}", encode_uri_component(id))
}

pub fn platform_label(platform: &str) -> String {
    let mut chars = platform.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
        None => String::new(),
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
