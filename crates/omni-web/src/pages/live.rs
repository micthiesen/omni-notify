//! `/live`: the full streamer roster, plus the live pieces Home reuses
//! (lead Stage and live rows). Rows are keyed by streamer id and read their
//! own memo, so counts update in place without re-creating rows.

use leptos::prelude::*;
use omni_api::intelligence::LivestreamIntelligence;
use omni_api::streamers::StreamerBinding;
use omni_api::streamers::{LiveStreamerView, OfflineStreamerView, StreamerTier, StreamerView};
use omni_web_kit::components::streamers::{
    DggPresenceTag, platform_label, preferred_watch, streamer_intelligence, streamer_path,
    viewer_number,
};
use omni_web_kit::components::{
    Avatar, ButtonLink, Delta, EmptyState, ErrorState, Glyph, Icon, IconSize, LiveTag, Meter,
    PageHead, Panel, PlatformIcon, Presence, Readout, ReadoutSize, SegOption, Segmented,
    SkeletonRows, Sparkline, Tag, TickNum, Tone,
};
use omni_web_kit::hooks::use_now;
use omni_web_kit::live::{LiveData, use_live_data};
use omni_web_kit::router::Link;
use omni_web_kit::utils::format::{format_compact_number, format_relative_at, format_uptime};
use omni_web_kit::utils::js::locale_number;

/// Live ordering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveSort {
    Relevance,
    Viewers,
}

pub fn sort_options() -> Vec<SegOption<LiveSort>> {
    vec![
        SegOption::new(LiveSort::Relevance, "Relevance"),
        SegOption::new(LiveSort::Viewers, "Viewers"),
    ]
}

/// Ids of live streamers: relevance (then viewers), or viewers only.
pub fn live_order(streamers: &[LiveStreamerView], sort: LiveSort) -> Vec<String> {
    let mut ranked: Vec<(f64, i64, &str)> = streamers
        .iter()
        .map(|s| {
            let relevance = streamer_intelligence(s).map_or(0.0, |i| i.relevance_score);
            (relevance, viewer_number(s), s.id.as_str())
        })
        .collect();
    ranked.sort_by(|a, b| match sort {
        LiveSort::Relevance => b.0.total_cmp(&a.0).then(b.1.cmp(&a.1)),
        LiveSort::Viewers => b.1.cmp(&a.1),
    });
    ranked.into_iter().map(|(_, _, id)| id.to_owned()).collect()
}

/// Live streamers in snapshot order.
pub fn live_list(live: LiveData) -> Vec<LiveStreamerView> {
    live.snapshot.with(|s| {
        s.as_ref()
            .map(|s| omni_web_kit::components::streamers::live_streamers(&s.streamers))
            .unwrap_or_default()
    })
}

/// The binding a Watch target opens, tracking binding changes.
pub fn use_watch_binding(
    streamer: Memo<Option<LiveStreamerView>>,
) -> Memo<Option<StreamerBinding>> {
    Memo::new(move |_| streamer.with(|s| s.as_ref().map(preferred_watch)))
}

/// "Watch Destiny on YouTube".
pub fn watch_label(name: &str, binding: Option<&StreamerBinding>) -> String {
    match binding {
        Some(b) => format!("Watch {name} on {}", platform_label(&b.platform)),
        None => format!("Watch {name}"),
    }
}

/// A keyed memo over one live streamer.
pub fn use_live_streamer(live: LiveData, id: String) -> Memo<Option<LiveStreamerView>> {
    Memo::new(move |_| {
        live.snapshot.with(|s| {
            s.as_ref().and_then(|s| {
                s.streamers.iter().find_map(|v| match v {
                    StreamerView::Live(l) if l.id == id => Some(l.clone()),
                    _ => None,
                })
            })
        })
    })
}

/// The order of ids, changing only when the order actually changes.
pub fn use_live_order(live: LiveData, sort: Signal<LiveSort>) -> Memo<Vec<String>> {
    Memo::new(move |_| {
        let sort = sort.get();
        live.snapshot.with(|s| {
            s.as_ref()
                .map(|s| {
                    live_order(
                        &omni_web_kit::components::streamers::live_streamers(&s.streamers),
                        sort,
                    )
                })
                .unwrap_or_default()
        })
    })
}

/// `"4 channels · 41,786 watching"`.
pub fn on_air_meta(streamers: &[LiveStreamerView]) -> String {
    let total: i64 = streamers.iter().map(viewer_number).sum();
    format!(
        "{} channel{} · {} watching",
        streamers.len(),
        if streamers.len() == 1 { "" } else { "s" },
        locale_number(total as f64)
    )
}

fn intel(s: &Option<LiveStreamerView>) -> Option<LivestreamIntelligence> {
    s.as_ref().and_then(streamer_intelligence)
}

/// Δ vs the 15-minute baseline and the backend surge gate.
fn trend_delta(i: &Option<LivestreamIntelligence>) -> (Option<f64>, bool) {
    i.as_ref()
        .and_then(|i| i.trend.as_ref())
        .map_or((None, false), |t| (Some(t.percent_change), t.anomalous))
}

/// One live channel row (Home's other channels, `/live` On air table). The
/// row opens the stream on the preferred live platform in a new tab; the
/// chevron at the end opens the streamer page.
#[component]
pub fn LiveRow(id: String, #[prop(optional)] with_meter: bool) -> impl IntoView {
    let live = use_live_data();
    let now = use_now(1000);
    let href = streamer_path(&id);
    let streamer = use_live_streamer(live, id);
    let intelligence = Memo::new(move |_| streamer.with(intel));
    let name = streamer.with_untracked(|s| {
        s.as_ref()
            .map(|s| s.display_name.clone())
            .unwrap_or_default()
    });
    let watch = use_watch_binding(streamer);
    let platform = watch.with_untracked(|w| w.as_ref().map(|w| w.platform.clone()));
    let watch_name = name.clone();
    let watch_aria = Signal::derive(move || watch.with(|w| watch_label(&watch_name, w.as_ref())));
    let details_aria = format!("{name} details");
    let viewers = Signal::derive(move || {
        streamer.with(|s| {
            s.as_ref()
                .map(|s| locale_number(viewer_number(s) as f64))
                .unwrap_or_default()
        })
    });
    let delta = Signal::derive(move || trend_delta(&intelligence.get()).0);
    let surge = Signal::derive(move || trend_delta(&intelligence.get()).1);
    let uptime = move || {
        streamer.with(|s| {
            s.as_ref()
                .map(|s| format_uptime(now.get() - s.started_at as f64))
                .unwrap_or_default()
        })
    };
    view! {
        <div class="row live-row">
        <a
            class="live-watch"
            href=move || watch.with(|w| w.as_ref().map(|w| w.url.clone()).unwrap_or_default())
            target="_blank"
            rel="noopener"
            aria-label=watch_aria
            title=watch_aria
        >
            <Avatar name=name.clone() size=32 presence=Presence::Live platform=platform.unwrap_or_default()/>
            <span class="row-main">
                <span class="row-title">{name.clone()}</span>
                <span class="row-sub truncate">{move || streamer.with(|s| s.as_ref().map(|s| s.title.clone()).unwrap_or_default())}</span>
            </span>
            <span class="row-end">
                <span class="num dim hide-phone">{uptime}</span>
                <span class="hide-phone">
                    {move || view! { <DggPresenceTag dgg=streamer.with(|s| s.as_ref().and_then(|s| s.dgg))/> }}
                </span>
                <Delta percent=delta surge/>
                {with_meter.then(|| {
                    let value = Signal::derive(move || streamer.with(|s| s.as_ref().map_or(0.0, |s| viewer_number(s) as f64)));
                    let max = Signal::derive(move || {
                        let peak = streamer.with(|s| s.as_ref().map_or(0.0, |s| s.max_viewer_count as f64));
                        let typical = intelligence.with(|i| i.as_ref().and_then(|i| i.trend.as_ref()?.typical_peak_viewers)).unwrap_or(0.0);
                        peak.max(typical).max(1.0)
                    });
                    let reference = Signal::derive(move || intelligence.with(|i| i.as_ref().and_then(|i| i.trend.as_ref()?.typical_peak_viewers)));
                    view! {
                        <span class="hide-phone">
                            <Meter value max reference width=96 label="Now against session peak"/>
                        </span>
                    }
                })}
                <TickNum value=viewers class="row-value readout-m"/>
                <Glyph icon=Icon::External size=IconSize::Small class="watch-hint"/>
            </span>
        </a>
        <Link to=href class="live-details" aria_label=details_aria.clone() title=details_aria>
            <Glyph icon=Icon::ChevronRight/>
        </Link>
        </div>
    }
}

/// The lead live channel as a Stage hero (Home).
#[component]
pub fn LeadStage(id: String) -> impl IntoView {
    let live = use_live_data();
    let now = use_now(1000);
    let href = streamer_path(&id);
    let history_id = id.clone();
    let streamer = use_live_streamer(live, id);
    let intelligence = Memo::new(move |_| streamer.with(intel));
    let s = move || streamer.get();
    let name = streamer.with_untracked(|s| {
        s.as_ref()
            .map(|s| s.display_name.clone())
            .unwrap_or_default()
    });
    let watch = use_watch_binding(streamer);
    let platform =
        watch.with_untracked(|w| w.as_ref().map(|w| w.platform.clone()).unwrap_or_default());
    let watch_name = name.clone();
    let watch_aria = Signal::derive(move || watch.with(|w| watch_label(&watch_name, w.as_ref())));
    let details_aria = format!("{name} details");
    let typical = Signal::derive(move || {
        intelligence.with(|i| {
            i.as_ref()
                .and_then(|i| i.trend.as_ref()?.typical_peak_viewers)
        })
    });
    let points = Signal::derive(move || {
        live.viewer_history.with(|h| {
            h.get(&history_id)
                .map(|v| v.iter().map(|(_, n)| *n).collect())
                .unwrap_or_default()
        })
    });
    let viewers = Signal::derive(move || {
        s().map(|s| locale_number(viewer_number(&s) as f64))
            .unwrap_or_default()
    });
    let peak = Signal::derive(move || {
        s().map(|s| locale_number(s.max_viewer_count as f64))
            .unwrap_or_default()
    });
    let delta = Signal::derive(move || trend_delta(&intelligence.get()).0);
    let surge = Signal::derive(move || trend_delta(&intelligence.get()).1);
    let chapter =
        move || intelligence.with(|i| i.as_ref().and_then(|i| i.chapters.last().cloned()));
    let tier_background = s().is_some_and(|s| s.tier == StreamerTier::Background);
    view! {
        <div class="lead">
            <div class="lead-head">
                <Avatar name=name.clone() size=64 presence=Presence::Live platform=platform.clone()/>
                <div class="lead-id">
                    <div class="cluster">
                        <Link to=href.clone() class="lead-name">{name.clone()}</Link>
                        {tier_background.then(|| view! { <Tag title="Notifications are muted for background streamers">"Background"</Tag> })}
                        <span class="num dim">{move || s().map(|s| format_uptime(now.get() - s.started_at as f64)).unwrap_or_default()}</span>
                    </div>
                    <h2 class="lead-title">{move || s().map(|s| s.title).unwrap_or_default()}</h2>
                </div>
            </div>
            <div class="lead-actions">
                <a
                    class="btn primary watch-btn"
                    href=move || watch.with(|w| w.as_ref().map(|w| w.url.clone()).unwrap_or_default())
                    target="_blank"
                    rel="noopener"
                    aria-label=watch_aria
                >
                    {move || watch.with(|w| w.as_ref().map(|w| view! { <PlatformIcon platform=w.platform.clone() size=16/> }))}
                    <span>{move || watch.with(|w| w.as_ref().map_or_else(|| "Watch".to_owned(), |w| format!("Watch on {}", platform_label(&w.platform))))}</span>
                    <Glyph icon=Icon::External size=IconSize::Small/>
                </a>
                <Link to=href.clone() class="btn" aria_label=details_aria>
                    "Details"
                    <Glyph icon=Icon::ChevronRight size=IconSize::Small/>
                </Link>
            </div>
            {move || chapter().map(|c| view! {
                <div class="lead-chapter">
                    <h3>{c.title}</h3>
                    <p class="clamp-2">{c.summary}</p>
                </div>
            })}
            <div class="lead-figures">
                <div class="lead-now">
                    <Readout label="Watching now" value=viewers size=ReadoutSize::Xl tone=Tone::Signal/>
                    <Sparkline points reference=typical tone=Tone::Signal height=56 label="Viewers this session"/>
                </div>
                <div class="lead-facts">
                    <Readout label="Session peak" value=peak size=ReadoutSize::M/>
                    <Readout label="vs 15 min ago" value=Signal::derive(move || {
                        delta.get().map(|d| format!("{}{:.1}%", if d >= 0.0 { "+" } else { "" }, d)).unwrap_or_else(|| "—".to_owned())
                    }) size=ReadoutSize::M>
                        <Delta percent=delta surge/>
                    </Readout>
                    <Readout label="DGG" value=Signal::derive(move || {
                        s().and_then(|s| s.dgg).map(|d| {
                            if d.hosted { "Hosted".to_owned() } else { d.viewers.map(|v| locale_number(v as f64)).unwrap_or_else(|| "Embedded".to_owned()) }
                        }).unwrap_or_else(|| "—".to_owned())
                    }) size=ReadoutSize::M/>
                    <Readout label="Typical peak" value=Signal::derive(move || {
                        typical.get().map(format_compact_number).unwrap_or_else(|| "—".to_owned())
                    }) size=ReadoutSize::M/>
                </div>
            </div>
        </div>
    }
}

/// Offline streamers grouped by tier.
fn offline_list(live: LiveData) -> Vec<OfflineStreamerView> {
    live.snapshot.with(|s| {
        s.as_ref()
            .map(|s| {
                s.streamers
                    .iter()
                    .filter_map(|v| match v {
                        StreamerView::Offline(o) => Some(o.clone()),
                        StreamerView::Live(_) => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    })
}

#[component]
pub fn OfflineRow(streamer: OfflineStreamerView, now: f64) -> impl IntoView {
    let last = streamer
        .last_ended_at
        .or(streamer.last_started_at)
        .map(|t| format_relative_at(t as f64, now));
    let platform = streamer
        .bindings
        .first()
        .map(|b| b.platform.clone())
        .unwrap_or_default();
    view! {
        <Link to=streamer_path(&streamer.id) class="row">
            <Avatar name=streamer.display_name.clone() size=32 presence=Presence::Offline platform=platform/>
            <span class="row-main">
                <span class="row-title">{streamer.display_name.clone()}</span>
                <span class="row-sub">{last.map_or_else(|| "Never seen live".to_owned(), |l| format!("Last live {l}"))}</span>
            </span>
            <span class="row-end">
                {streamer.last_max_viewer_count.map(|p| view! {
                    <span class="num dim" title="Last session peak">{format!("peak {}", locale_number(p as f64))}</span>
                })}
            </span>
        </Link>
    }
}

#[component]
pub fn LivePage() -> impl IntoView {
    let live = use_live_data();
    let sort = RwSignal::new(LiveSort::Relevance);
    let order = use_live_order(live, sort.into());
    let total = Memo::new(move |_| {
        live.snapshot
            .with(|s| s.as_ref().map_or(0, |s| s.streamers.len()))
    });
    let meta = Memo::new(move |_| on_air_meta(&live_list(live)));
    let title = Signal::derive(move || {
        let n = order.with(Vec::len);
        if live.snapshot.with(Option::is_none) {
            "Live".to_owned()
        } else if n == 0 {
            "Nobody on air.".to_owned()
        } else {
            format!("{n} of {} on air.", total.get())
        }
    });
    let now = use_now(60_000);
    // Gate on loaded-ness only: reading the snapshot here would rebuild the
    // roster, its sort control and every row on each refresh.
    let loaded = Memo::new(move |_| live.snapshot.with(Option::is_some));
    view! {
        <PageHead
            title
            sentence=true
            lede=Signal::derive(move || (order.with(|o| !o.is_empty())).then(|| meta.get()))
            actions=ViewFn::from(|| view! {
                <ButtonLink to="/live/streamers" icon=Icon::Pencil title="Add, edit and reorder tracked streamers">
                    "Manage streamers"
                </ButtonLink>
            })
        />
        {move || {
            if !loaded.get() {
                return match live.error.get() {
                    Some(e) => view! { <ErrorState title="Could not load streamers" raw=e page=true/> }.into_any(),
                    None => view! { <SkeletonRows count=6/> }.into_any(),
                };
            }
            view! {
                <div class="stack-lg">
                    <Panel
                        title="On air"
                        stage=Signal::derive(move || order.with(|o| !o.is_empty()))
                        head_end=ViewFn::from(move || view! {
                            <Segmented
                                options=Signal::derive(sort_options)
                                value=sort
                                on_change=Callback::new(move |v| sort.set(v))
                                aria_label="Sort live channels"
                                small=true
                            />
                        })
                    >
                        {move || {
                            if order.with(Vec::is_empty) {
                                view! { <EmptyState icon=Icon::Live message="Nobody is live right now." compact=true/> }.into_any()
                            } else {
                                view! {
                                    <div class="rows" data-primary-rows="true">
                                        <For each=move || order.get() key=|id| id.clone() children=|id| view! {
                                            <LiveRow id with_meter=true/>
                                        }/>
                                    </div>
                                }.into_any()
                            }
                        }}
                    </Panel>
                    {[(StreamerTier::Primary, "Offline · primary"), (StreamerTier::Background, "Offline · background")]
                        .into_iter()
                        .map(|(tier, title)| {
                            let rows = Memo::new(move |_| {
                                let mut list: Vec<_> = offline_list(live).into_iter().filter(|o| o.tier == tier).collect();
                                list.sort_by_key(|o| std::cmp::Reverse(o.last_ended_at.or(o.last_started_at).unwrap_or(0)));
                                list
                            });
                            move || (!rows.with(Vec::is_empty)).then(|| view! {
                                <Panel title=title>
                                    <div class="rows">
                                        {move || rows.get().into_iter().map(|streamer| view! { <OfflineRow streamer now=now.get()/> }).collect_view()}
                                    </div>
                                </Panel>
                            })
                        })
                        .collect_view()}
                    <p class="small dim">
                        <LiveTag/>" counts update every few seconds; "
                        {move || format_compact_number(order.with(Vec::len) as f64)}
                        " live of "
                        {move || total.get()}
                        " tracked."
                    </p>
                </div>
            }
            .into_any()
        }}
    }
}

#[cfg(test)]
mod tests {
    use omni_api::streamers::{LiveTrue, StreamerBinding};

    use super::*;

    fn streamer(id: &str, viewers: i64) -> LiveStreamerView {
        let binding = StreamerBinding {
            platform: "twitch".into(),
            username: id.into(),
            url: String::new(),
        };
        LiveStreamerView {
            id: id.into(),
            display_name: id.into(),
            bindings: vec![binding.clone()],
            tier: StreamerTier::Primary,
            dgg: None,
            live: LiveTrue,
            title: String::new(),
            started_at: 0,
            max_viewer_count: viewers,
            viewer_count: Some(viewers),
            sources: Vec::new(),
            category: None,
            primary: binding,
            intelligence: None,
        }
    }

    #[test]
    fn viewers_break_relevance_ties() {
        let list = [streamer("a", 10), streamer("b", 300), streamer("c", 20)];
        assert_eq!(live_order(&list, LiveSort::Relevance), ["b", "c", "a"]);
        assert_eq!(live_order(&list, LiveSort::Viewers), ["b", "c", "a"]);
        assert_eq!(on_air_meta(&list[..1]), "1 channel · 10 watching");
    }
}
