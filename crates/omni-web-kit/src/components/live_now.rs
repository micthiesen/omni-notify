//! Live streamer cards and the offline strip.
//! Cards are keyed by streamer id and read their own memo, so viewer counts,
//! titles and status update in place from each snapshot.

use leptos::prelude::*;
use omni_api::intelligence::{LivestreamIntelligence, PresenceState};
use omni_api::streamers::{DggPresence, LiveStreamerView, OfflineStreamerView, StreamerView};

use super::platform_icon::PlatformIcon;
use crate::hooks::use_now;
use crate::router::Link;
use crate::utils::format::{
    format_compact_number, format_duration, format_relative, format_uptime,
};
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
        let class = if dgg.hosted {
            "dgg-tag dgg-tag-hosted"
        } else {
            "dgg-tag"
        };
        view! { <span class=class>{label}</span> }
    })
}

#[component]
fn LiveStreamerCard(#[prop(into)] streamer: Signal<LiveStreamerView>) -> impl IntoView {
    let now = use_now(1000);
    let intelligence = Memo::new(move |_| streamer.with(streamer_intelligence));
    let s = move || streamer.get();
    view! {
        <div class="live-card">
            <div class="live-card-header">
                {move || view! { <PlatformIcon platform=s().primary.platform size=16/> }}
                <span class="live-name">{move || s().display_name}</span>
                <span class="live-badge">
                    <span class="live-badge-dot"></span>
                    "LIVE"
                </span>
            </div>
            <div class="live-title">{move || s().title}</div>
            {move || {
                intelligence
                    .get()
                    .and_then(|i| i.summary)
                    .map(|summary| {
                        view! {
                            <div class="live-now-summary">
                                <strong>"Now:"</strong>
                                " "
                                {summary.text}
                            </div>
                        }
                    })
            }}
            <div class="meta-row live-meta">
                <span class="live-uptime">
                    {move || format_uptime(now.get() - streamer.with(|s| s.started_at) as f64)}
                </span>
                {move || streamer.with(viewers_label).map(|v| view! { <span>{v}</span> })}
                {move || s().category.map(|c| view! { <span>{c}</span> })}
                {move || view! { <DggPresenceTag dgg=s().dgg/> }}
                {move || {
                    intelligence
                        .get()
                        .and_then(|i| i.destiny_presence)
                        .filter(|p| p.state == PresenceState::Confirmed)
                        .map(|_| {
                            view! {
                                <span class="intelligence-badge intelligence-badge-destiny">
                                    "Destiny Here"
                                </span>
                            }
                        })
                }}
                {move || {
                    intelligence
                        .get()
                        .filter(|i| i.relevance_score >= 70.0)
                        .map(|i| {
                            view! {
                                <span class="intelligence-badge">
                                    {format!("Relevance {}", crate::utils::js::number_string(i.relevance_score))}
                                </span>
                            }
                        })
                }}
            </div>
            // Stretched-link pattern: the whole card opens the stream; the compact
            // details control above it routes to the streamer detail page.
            <a
                class="live-card-overlay"
                href=move || s().primary.url
                target="_blank"
                rel="noopener"
                aria-label=move || {
                    let s = s();
                    format!(
                        "Watch {} on {} (opens in a new tab)",
                        s.display_name,
                        platform_label(&s.primary.platform),
                    )
                }
            ></a>
            {move || {
                let s = s();
                view! {
                    <Link
                        class="live-card-details"
                        to=streamer_path(&s.id)
                        aria_label=format!("View details for {}", s.display_name)
                    >
                        <span aria-hidden="true">"Details ›"</span>
                    </Link>
                }
            }}
        </div>
    }
}

fn offline_title(streamer: &OfflineStreamerView) -> String {
    match (streamer.last_ended_at, streamer.last_started_at) {
        (Some(ended), Some(started)) => {
            let peak = match streamer.last_max_viewer_count {
                Some(peak) if peak != 0 => {
                    format!(", peak {} viewers", format_compact_number(peak as f64))
                }
                _ => String::new(),
            };
            format!(
                "Last live {} for {}{peak}",
                format_relative(ended as f64),
                format_duration((ended - started) as f64)
            )
        }
        _ => "No streams seen yet".to_owned(),
    }
}

#[component]
fn OfflinePill(#[prop(into)] streamer: Signal<OfflineStreamerView>) -> impl IntoView {
    move || {
        let streamer = streamer.get();
        let last_live = streamer
            .last_ended_at
            .map(|ended| format_relative(ended as f64));
        view! {
            <Link class="offline-pill" to=streamer_path(&streamer.id) title=offline_title(&streamer)>
                {streamer
                    .bindings
                    .first()
                    .map(|b| view! { <PlatformIcon platform=b.platform.clone() size=12/> })}
                <span class="offline-name">{streamer.display_name.clone()}</span>
                {last_live.map(|when| view! { <span class="offline-when">{when}</span> })}
            </Link>
        }
    }
}

/// The live entry `id` of `streamers`, falling back to `previous` while the
/// keyed list catches up.
fn find_live(all: &[StreamerView], id: &str) -> Option<LiveStreamerView> {
    all.iter().find_map(|s| match s {
        StreamerView::Live(live) if live.id == id => Some(live.clone()),
        _ => None,
    })
}

fn find_offline(all: &[StreamerView], id: &str) -> Option<OfflineStreamerView> {
    all.iter().find_map(|s| match s {
        StreamerView::Offline(offline) if offline.id == id => Some(offline.clone()),
        _ => None,
    })
}

/// The server's ordering is kept as is (the snapshot and the iOS live-slot API
/// share one ranking).
#[component]
pub fn LiveNow(#[prop(into)] streamers: Signal<Vec<StreamerView>>) -> impl IntoView {
    let live_list = Memo::new(move |_| {
        streamers.with(|all| {
            all.iter()
                .filter_map(|s| match s {
                    StreamerView::Live(live) => Some(live.clone()),
                    StreamerView::Offline(_) => None,
                })
                .collect::<Vec<_>>()
        })
    });
    let offline_list = Memo::new(move |_| {
        streamers.with(|all| {
            all.iter()
                .filter_map(|s| match s {
                    StreamerView::Offline(offline) => Some(offline.clone()),
                    StreamerView::Live(_) => None,
                })
                .collect::<Vec<_>>()
        })
    });
    let has_any = Memo::new(move |_| streamers.with(|s| !s.is_empty()));
    let live_card = move |initial: LiveStreamerView| {
        let id = initial.id.clone();
        let current = Memo::new(move |previous: Option<&LiveStreamerView>| {
            streamers
                .with(|all| find_live(all, &id))
                .or_else(|| previous.cloned())
                .unwrap_or_else(|| initial.clone())
        });
        view! { <LiveStreamerCard streamer=current/> }
    };
    let offline_pill = move |initial: OfflineStreamerView| {
        let id = initial.id.clone();
        let current = Memo::new(move |previous: Option<&OfflineStreamerView>| {
            streamers
                .with(|all| find_offline(all, &id))
                .or_else(|| previous.cloned())
                .unwrap_or_else(|| initial.clone())
        });
        view! { <OfflinePill streamer=current/> }
    };
    move || {
        has_any.get().then(|| {
            view! {
                <section class="page-section live-now-section">
                    <h2 class="section-title">
                        "Live Now"
                        {move || {
                            let count = live_list.with(Vec::len);
                            (count > 0)
                                .then(|| {
                                    view! {
                                        <span class="section-count live-count">
                                            {format!("{count} live")}
                                        </span>
                                    }
                                })
                        }}
                    </h2>
                    <Show
                        when=move || live_list.with(|l| !l.is_empty())
                        fallback=|| view! { <div class="muted live-empty">"No one is live right now."</div> }
                    >
                        <div class="live-grid">
                            <For each=move || live_list.get() key=|s| s.id.clone() children=live_card/>
                        </div>
                    </Show>
                    <Show when=move || offline_list.with(|l| !l.is_empty())>
                        <details class="offline-disclosure">
                            <summary>
                                "Following "
                                <span class="muted">
                                    {move || format!("{} Offline", offline_list.with(Vec::len))}
                                </span>
                            </summary>
                            <div class="offline-strip">
                                <For each=move || offline_list.get() key=|s| s.id.clone() children=offline_pill/>
                            </div>
                        </details>
                    </Show>
                </section>
            }
        })
    }
}
