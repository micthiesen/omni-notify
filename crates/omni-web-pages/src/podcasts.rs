//! Podcast picks: the picks wall with its inspector, and one pick's detail
//! page. Shares its look with Movies & TV (`rec_ui`, the `.pick*` and
//! `.rec-*` styles) with square artwork.

use std::time::Duration;

use leptos::prelude::*;
use omni_api::common::encode_uri_component;
use omni_api::podcasts::{
    PodcastFeedback, PodcastQueueResult, PodcastRecommendation, PodcastRecommendationStatus,
    PodcastShortlistScores, PodcastTasteProfile,
};
use omni_web_kit::api;
use omni_web_kit::chrome::use_page_label;
use omni_web_kit::components::{
    Button, ButtonLink, ButtonSize, ButtonVariant, EmptyState, ErrorState, Icon, Inspector, Meter,
    PageHead, Panel, Poster, RunButton, SegOption, Segmented, ShowMoreButton, Skeleton,
    SkeletonKind, Status, StatusKind, Tag, ToastKind, Tone, use_toast,
};
use omni_web_kit::hooks::{query_param, scroll_into_view_center};
use omni_web_kit::live::use_live_data;
use omni_web_kit::router::Link;
use omni_web_kit::task::{sleep, spawn_detached, spawn_scoped};
use omni_web_kit::utils::format::{format_absolute_with_year, format_date_only, format_relative};
use omni_web_kit::utils::js::{js_round, number_string};
use omni_web_kit::utils::rec_labels::{PODCAST_STATUS_ORDER, podcast_status_label};

use crate::common::format_minutes;
use crate::rec_ui::{Choice, ChoiceGlyph, ChoiceStyle, Choices, MediaSwitch};
use crate::taste_brain::{TasteBrain, TasteBrainProfile};

const TASK_NAME: &str = "PodcastRecs";
const TASTE_TASK_NAME: &str = "PodcastTasteReflection";
const MAX_PICKS: u32 = 5;
const PAGE: usize = 24;
/// Picks in the "Up next" rail.
const UP_NEXT: usize = 4;

/// The podcast rating options, in display order.
fn podcast_choices() -> Vec<Choice<PodcastFeedback>> {
    vec![
        Choice {
            value: PodcastFeedback::GoodPick,
            label: "Good pick",
            glyph: ChoiceGlyph::Up,
        },
        Choice {
            value: PodcastFeedback::NotForMe,
            label: "Not for me",
            glyph: ChoiceGlyph::Down,
        },
    ]
}

fn feedback_label(feedback: PodcastFeedback) -> &'static str {
    podcast_choices()
        .into_iter()
        .find(|choice| choice.value == feedback)
        .map_or("", |choice| choice.label)
}

/// Feedback opens once a pick has been delivered.
fn can_rate(status: PodcastRecommendationStatus) -> bool {
    status != PodcastRecommendationStatus::Pending && status != PodcastRecommendationStatus::Failed
}

fn in_castro_queue(rec: &PodcastRecommendation) -> bool {
    matches!(
        rec.queue_result,
        Some(PodcastQueueResult::Queued | PodcastQueueResult::AlreadyQueued)
    )
}

/// The status shape and word for a pick. A notified pick that reached the
/// Castro queue reads "In Castro" (the queued state, an info ring).
pub(crate) fn pick_status(rec: &PodcastRecommendation) -> (StatusKind, &'static str) {
    match rec.status {
        PodcastRecommendationStatus::Notified if in_castro_queue(rec) => {
            (StatusKind::Info, "In Castro")
        }
        PodcastRecommendationStatus::Listened => (StatusKind::Ok, "Listened"),
        PodcastRecommendationStatus::Failed => (StatusKind::Fault, "Failed"),
        status => (StatusKind::Idle, podcast_status_label(status)),
    }
}

fn featuring(rec: &PodcastRecommendation) -> Option<String> {
    (!rec.matched_voices.is_empty()).then(|| format!("Featuring {}", rec.matched_voices.join(", ")))
}

fn pick_path(id: &str) -> String {
    format!("/podcasts/{}", encode_uri_component(id))
}

/// "13 waiting in Castro · 2 listened" under the title.
pub fn podcasts_lede(recs: &[PodcastRecommendation]) -> String {
    let queued = recs
        .iter()
        .filter(|r| r.status == PodcastRecommendationStatus::Notified && in_castro_queue(r))
        .count();
    let listened = recs
        .iter()
        .filter(|r| r.status == PodcastRecommendationStatus::Listened)
        .count();
    let mut parts = vec![match queued {
        0 => "Nothing waiting in Castro".to_owned(),
        n => format!("{n} waiting in Castro"),
    }];
    if listened > 0 {
        parts.push(format!("{listened} listened"));
    }
    parts.join(" · ")
}

/// The picks shown for `filter` (`None` = every status), in list order.
fn filtered(
    recs: &[PodcastRecommendation],
    filter: Option<PodcastRecommendationStatus>,
) -> impl Iterator<Item = &PodcastRecommendation> {
    recs.iter()
        .filter(move |r| filter.is_none_or(|status| r.status == status))
}

/// How many cards to reveal so the pick at `index` is visible.
fn limit_to_show(index: usize) -> usize {
    (index / PAGE + 1) * PAGE
}

fn status_line(rec: &PodcastRecommendation) -> impl IntoView + use<> {
    let (kind, word) = pick_status(rec);
    let title = in_castro_queue(rec).then(|| "This episode is in your Castro queue".to_owned());
    view! { <Status kind=kind label=word.to_owned() title=title/> }
}

fn caveat_list(items: &[String]) -> impl IntoView + use<> {
    view! {
        <ul class="rec-caveats">
            {items.iter().map(|c| view! { <li>{c.clone()}</li> }).collect_view()}
        </ul>
    }
}

/// Shortlist scores as Meters; the top score takes Signal.
fn score_rows(scores: &PodcastShortlistScores) -> impl IntoView + use<> {
    let rows = [
        ("Taste match", scores.taste_match),
        ("Novelty", scores.novelty),
        ("Composite", scores.composite),
    ];
    let top = rows.iter().map(|(_, v)| *v).fold(f64::MIN, f64::max);
    let risks = (!scores.risks.is_empty()).then(|| {
        view! {
            <div class="rec-risks">
                <h4 class="label">"Risks"</h4>
                {caveat_list(&scores.risks)}
            </div>
        }
    });
    view! {
        <div class="rec-scores">
            {rows
                .into_iter()
                .map(|(label, value)| {
                    let tone = if value == top { Tone::Signal } else { Tone::Neutral };
                    view! {
                        <div class="rec-score">
                            <span class="small">{label}</span>
                            <Meter value=value.clamp(0.0, 100.0) max=100.0 tone=tone label=format!("{label} score")/>
                            <span class="num">{number_string(js_round(value))}</span>
                        </div>
                    }
                })
                .collect_view()}
        </div>
        {risks}
    }
}

fn timeline(rec: &PodcastRecommendation) -> impl IntoView + use<> {
    let mut events = vec![("Recommended".to_owned(), rec.recommended_at)];
    if let Some(at) = rec.notified_at {
        events.push(("Notified".to_owned(), at));
    }
    if let (Some(feedback), Some(at)) = (rec.feedback, rec.feedback_at) {
        events.push((format!("Feedback · {}", feedback_label(feedback)), at));
    }
    view! {
        <ol class="rec-timeline">
            {events
                .into_iter()
                .map(|(label, at)| view! {
                    <li>
                        <span>{label}</span>
                        <time class="num">{format_absolute_with_year(at as f64)}</time>
                    </li>
                })
                .collect_view()}
        </ol>
    }
}

/// Facts as key-value pairs; absent fields are skipped.
fn pick_facts(rec: &PodcastRecommendation) -> Vec<(&'static str, String)> {
    let mut fields = vec![("Released", format_date_only(rec.published_at as f64))];
    if let Some(minutes) = rec.duration_minutes {
        fields.push(("Length", format_minutes(minutes as f64)));
    }
    if let Some(via) = rec.discovered_via.clone().filter(|v| !v.is_empty()) {
        fields.push(("Found via", via));
    }
    if let Some(confidence) = rec.confidence {
        fields.push((
            "Confidence",
            format!("{}%", number_string(js_round(confidence * 100.0))),
        ));
    }
    if let Some(itunes) = rec.itunes_id {
        fields.push(("iTunes ID", itunes.to_string()));
    }
    fields
}

fn fact_list(rec: &PodcastRecommendation) -> impl IntoView + use<> {
    view! {
        <dl class="kv rec-facts">
            {pick_facts(rec)
                .into_iter()
                .map(|(label, value)| view! { <dt>{label}</dt><dd>{value}</dd> })
                .collect_view()}
        </dl>
    }
}

fn episode_links(rec: &PodcastRecommendation, size: ButtonSize) -> impl IntoView + use<> {
    let episode_url = rec.episode_url.clone().filter(|u| !u.is_empty());
    let source_url = rec.source_url.clone().filter(|u| !u.is_empty());
    let title = rec.episode_title.clone();
    view! {
        {episode_url.map(|url| view! {
            <ButtonLink to=url external=true size aria_label=format!("Open {title}")>
                "Open episode"
            </ButtonLink>
        })}
        {source_url.map(|url| view! {
            <ButtonLink to=url external=true size variant=ButtonVariant::Ghost>
                "Discussion"
            </ButtonLink>
        })}
    }
}

/// Show title and featured voices under an episode title.
fn show_line(rec: &PodcastRecommendation) -> impl IntoView + use<> {
    view! {
        <span class="pod-show">{rec.show_title.clone()}</span>
        {featuring(rec).map(|f| view! { <span class="pod-voices">{f}</span> })}
    }
}

/// A square-artwork card on the picks wall. Artwork and title open the
/// inspector.
#[component]
fn PickCard(
    rec: PodcastRecommendation,
    #[prop(into)] saving: Signal<bool>,
    highlighted: bool,
    on_open: Callback<()>,
    on_feedback: Callback<PodcastFeedback>,
) -> impl IntoView {
    let why = rec.why_for_user.clone().filter(|w| !w.is_empty());
    let duration = rec.duration_minutes.map(|m| format_minutes(m as f64));
    view! {
        <article
            id=format!("recommendation-{}", rec.recommendation_id)
            class=if highlighted { "pick deep-link-target" } else { "pick" }
        >
            <button
                type="button"
                class="pick-hit"
                aria-label=format!("Details for {}", rec.episode_title)
                on:click=move |_| on_open.run(())
            >
                <Poster src=rec.artwork_url.clone() title=rec.show_title.clone() square=true/>
                <span class="pick-title clamp-2">{rec.episode_title.clone()}</span>
            </button>
            {show_line(&rec)}
            <div class="pick-meta">
                {status_line(&rec)}
                {duration.map(|d| view! { <span class="num muted">{d}</span> })}
            </div>
            {why.map(|why| view! { <p class="pick-why">{why}</p> })}
            {can_rate(rec.status).then(|| view! {
                <Choices
                    choices=podcast_choices()
                    current=Signal::stored(rec.feedback)
                    saving
                    on_pick=on_feedback
                    style=ChoiceStyle::Compact
                    aria_label=format!("Rate {}", rec.episode_title)
                />
            })}
        </article>
    }
}

/// The selected pick in a drawer (bottom sheet on phone).
#[component]
fn PickInspector(
    #[prop(into)] rec: Signal<Option<PodcastRecommendation>>,
    #[prop(into)] saving: Signal<bool>,
    on_feedback: Callback<PodcastFeedback>,
    on_close: Callback<()>,
) -> impl IntoView {
    let title = Signal::derive(move || {
        rec.with(|r| {
            r.as_ref()
                .map(|r| r.episode_title.clone())
                .unwrap_or_default()
        })
    });
    let status = ViewFn::from(move || rec.with(|r| r.as_ref().map(status_line)));
    let actions = ViewFn::from(move || {
        rec.with(|r| {
            r.as_ref().map(|r| {
                view! {
                    {episode_links(r, ButtonSize::Sm)}
                    <ButtonLink to=pick_path(&r.recommendation_id) size=ButtonSize::Sm variant=ButtonVariant::Ghost>
                        "Open page"
                    </ButtonLink>
                }
            })
        })
    });
    let rec_id = Memo::new(move |_| rec.with(|r| r.as_ref().map(|r| r.recommendation_id.clone())));
    let current = Signal::derive(move || rec.with(|r| r.as_ref().and_then(|r| r.feedback)));
    let rateable = Memo::new(move |_| rec.with(|r| r.as_ref().is_some_and(|r| can_rate(r.status))));
    view! {
        <Inspector title status actions on_close>
            {move || {
                // Rebuilt only when another pick is selected, so focus stays
                // put through a rating save.
                rec_id.track();
                rec.get_untracked().map(|r| {
                    let why = r.why_for_user.clone().filter(|w| !w.is_empty());
                    let recommended = r.recommended_at as f64;
                    view! {
                        <div class="rec-inspector-art">
                            <Poster src=r.artwork_url.clone() title=r.show_title.clone() square=true eager=true/>
                            <div class="pod-inspector-lede">
                                {show_line(&r)}
                                <span class="small muted">{format!("Picked {}", format_relative(recommended))}</span>
                            </div>
                        </div>
                        {why.map(|why| view! {
                            <section class="inspector-section">
                                <p class="rec-why">{why}</p>
                            </section>
                        })}
                        {(!r.caveats.is_empty()).then(|| view! {
                            <section class="inspector-section">
                                <h3>"Before you listen"</h3>
                                {caveat_list(&r.caveats)}
                            </section>
                        })}
                        {move || rateable.get().then(|| view! {
                            <section class="inspector-section rec-take">
                                <h3>"Your take"</h3>
                                <Choices
                                    choices=podcast_choices()
                                    current
                                    saving
                                    on_pick=on_feedback
                                    aria_label="Rate this pick"
                                />
                            </section>
                        })}
                        {r.shortlist_scores.as_ref().map(|s| view! {
                            <section class="inspector-section">
                                <h3>"Shortlist scores"</h3>
                                {score_rows(s)}
                            </section>
                        })}
                        <section class="inspector-section">
                            <h3>"Details"</h3>
                            {fact_list(&r)}
                        </section>
                        <section class="inspector-section">
                            <h3>"Timeline"</h3>
                            {move || rec.with(|r| r.as_ref().map(timeline))}
                        </section>
                    }
                })
            }}
        </Inspector>
    }
}

fn podcast_stats(profile: &PodcastTasteProfile) -> Vec<(String, String)> {
    let stats = &profile.stats;
    vec![
        (
            "Episodes finished".into(),
            stats.listened_episodes.to_string(),
        ),
        (
            "Episodes started".into(),
            stats.started_episodes.to_string(),
        ),
        ("Starred".into(), stats.starred_episodes.to_string()),
        ("Shows heard".into(), stats.distinct_shows.to_string()),
        (
            "Picks listened".into(),
            format!(
                "{}/{}",
                stats.recommendations.listened, stats.recommendations.total
            ),
        ),
        ("Good picks".into(), stats.feedback.good_pick.to_string()),
    ]
}

#[derive(Clone, Debug, PartialEq)]
enum WallState {
    Loading,
    Error(String),
    Empty,
    NoMatch,
    List,
}

/// `/podcasts`.
#[component]
pub fn PodcastsPage() -> impl IntoView {
    let recs = RwSignal::new(None::<Vec<PodcastRecommendation>>);
    let recs_error = RwSignal::new(None::<String>);
    let reload = RwSignal::new(0u32);
    let status_filter = RwSignal::new(None::<PodcastRecommendationStatus>);
    let limit = RwSignal::new(PAGE);
    let saving_id = RwSignal::new(None::<String>);
    let selected = RwSignal::new(None::<String>);
    let picks = RwSignal::new(1u32);
    let taste_profile = RwSignal::new(None::<PodcastTasteProfile>);
    let taste_loading = RwSignal::new(true);
    let taste_error = RwSignal::new(None::<String>);
    let toast = use_toast();
    let live = use_live_data();

    let task_found = Memo::new(move |_| {
        live.snapshot.with(|s| {
            s.as_ref().map(|s| {
                s.tasks
                    .iter()
                    .find(|t| t.name == TASK_NAME)
                    .map(|t| t.running)
            })
        })
    });
    let running = Signal::derive(move || task_found.get().flatten().unwrap_or(false));
    let available = Memo::new(move |_| match task_found.get() {
        None => true,
        Some(found) => found.is_some(),
    });
    let latest_taste_run_id = Memo::new(move |_| {
        live.snapshot.with(|s| {
            s.as_ref()
                .and_then(|s| s.runs.iter().find(|r| r.task_name == TASTE_TASK_NAME))
                .map(|r| r.run_id.clone())
        })
    });

    // Load once, then reload whenever a reflection run lands.
    Effect::new(move |_| {
        latest_taste_run_id.track();
        spawn_scoped(async move {
            match api::fetch_podcast_taste_profile().await {
                Ok(data) => {
                    taste_profile.set(data.profile);
                    taste_error.set(None);
                }
                Err(err) => taste_error.set(Some(err.message().to_owned())),
            }
            taste_loading.set(false);
        });
    });

    // Load on mount, on retry, and again whenever a recommendation run ends.
    Effect::new(move |_| {
        reload.track();
        if running.get() {
            return;
        }
        spawn_scoped(async move {
            match api::fetch_podcast_recommendations().await {
                Ok(data) => {
                    recs.set(Some(data.recommendations));
                    recs_error.set(None);
                }
                Err(err) => recs_error.set(Some(err.message().to_owned())),
            }
        });
    });

    // `?recommendation=<id>`: switch to the pick's status, reveal it, scroll
    // to it and pulse it (once, on first load).
    let highlight = query_param("recommendation");
    let target = highlight.clone();
    Effect::new(move |done: Option<bool>| {
        if done == Some(true) {
            return true;
        }
        let Some(id) = target.clone() else {
            return true;
        };
        let found = recs.with(|list| {
            list.as_ref().map(|list| {
                list.iter()
                    .find(|r| r.recommendation_id == id)
                    .map(|r| r.status)
            })
        });
        let Some(found) = found else { return false };
        if let Some(status) = found {
            let index = recs.with(|list| {
                list.as_deref().map_or(0, |list| {
                    filtered(list, Some(status))
                        .position(|r| r.recommendation_id == id)
                        .unwrap_or(0)
                })
            });
            status_filter.set(Some(status));
            limit.set(limit_to_show(index));
            spawn_detached(async move {
                sleep(Duration::from_millis(60)).await;
                scroll_into_view_center(&format!("recommendation-{id}"));
            });
        }
        true
    });

    let save = move |recommendation_id: String, feedback: PodcastFeedback| {
        saving_id.set(Some(recommendation_id.clone()));
        spawn_detached(async move {
            match api::send_podcast_recommendation_feedback(
                &recommendation_id,
                Some(feedback),
                None,
            )
            .await
            {
                Ok(result) => {
                    recs.update(|list| {
                        if let Some(list) = list {
                            for rec in list.iter_mut() {
                                if rec.recommendation_id == recommendation_id {
                                    *rec = result.recommendation.clone();
                                }
                            }
                        }
                    });
                    toast.show("Feedback saved", ToastKind::Info);
                }
                Err(err) => toast.show(err.message(), ToastKind::Error),
            }
            saving_id.set(None);
        });
    };

    let total = Memo::new(move |_| recs.with(|r| r.as_ref().map_or(0, Vec::len)));
    let filter_options = Signal::derive(move || {
        recs.with(|list| {
            let list = list.as_deref().unwrap_or_default();
            let mut options = vec![SegOption::new(None, "All").with_count(list.len())];
            options.extend(PODCAST_STATUS_ORDER.iter().map(|status| {
                SegOption::new(Some(*status), podcast_status_label(*status))
                    .with_count(filtered(list, Some(*status)).count())
            }));
            options
        })
    });
    let visible = Memo::new(move |_| {
        let filter = status_filter.get();
        recs.with(|list| {
            list.as_deref()
                .map(|list| filtered(list, filter).cloned().collect::<Vec<_>>())
        })
    });
    let shown = Memo::new(move |_| {
        let n = limit.get();
        visible.with(|v| {
            v.as_deref()
                .map(|v| v.iter().take(n).cloned().collect::<Vec<_>>())
                .unwrap_or_default()
        })
    });
    let remaining = Signal::derive(move || {
        visible
            .with(|v| v.as_ref().map_or(0, Vec::len))
            .saturating_sub(limit.get())
    });
    let up_next = Memo::new(move |_| {
        recs.with(|list| {
            list.iter()
                .flatten()
                .filter(|r| r.status == PodcastRecommendationStatus::Notified)
                .take(UP_NEXT)
                .cloned()
                .collect::<Vec<_>>()
        })
    });
    // Only emptiness may rebuild the rail (and reset its scroll); feedback
    // saves update its items in place.
    let has_up_next = Memo::new(move |_| up_next.with(|items| !items.is_empty()));
    let selected_rec = Signal::derive(move || {
        let id = selected.get()?;
        recs.with(|list| {
            list.as_ref()?
                .iter()
                .find(|r| r.recommendation_id == id)
                .cloned()
        })
    });
    let lede = Signal::derive(move || recs.with(|r| r.as_deref().map(podcasts_lede)));

    let taste =
        Signal::derive(move || taste_profile.with(|p| p.as_ref().map(TasteBrainProfile::from)));
    let taste_stats = Signal::derive(move || {
        taste_profile.with(|p| p.as_ref().map(podcast_stats).unwrap_or_default())
    });

    let picks_select = (1..=MAX_PICKS)
        .map(|n| view! { <option value=n.to_string() prop:selected=move || picks.get() == n>{n}</option> })
        .collect_view();
    let actions = ViewFn::from(move || {
        if available.get() {
            view! {
                <label class="picks-count">
                    <span class="label">"Picks"</span>
                    <select
                        class="select"
                        aria-label="Maximum podcast recommendations"
                        disabled=move || running.get()
                        on:change=move |event| picks.set(event_target_value(&event).parse().unwrap_or(1))
                    >
                        {picks_select.clone()}
                    </select>
                </label>
                <RunButton
                    task=TASK_NAME
                    running
                    label="Find picks"
                    primary=true
                    max_recommendations=Signal::derive(move || Some(picks.get()))
                />
            }
            .into_any()
        } else {
            view! {
                <Button
                    icon=Icon::Play
                    disabled=true
                    disabled_reason="Task disabled: missing podcast recommendation configuration"
                >
                    "Find picks"
                </Button>
            }
            .into_any()
        }
    });

    let wall_state = Memo::new(move |_| {
        if recs.with(Option::is_none) {
            return match recs_error.get() {
                Some(error) => WallState::Error(error),
                None => WallState::Loading,
            };
        }
        if total.get() == 0 {
            WallState::Empty
        } else if visible.with(|v| v.as_ref().is_some_and(Vec::is_empty)) {
            WallState::NoMatch
        } else {
            WallState::List
        }
    });
    let highlight_id = StoredValue::new(highlight);

    let wall = move || {
        let state = wall_state.get();
        if let WallState::Error(error) = &state {
            return view! {
                <ErrorState
                    title="Podcast picks could not load"
                    raw=error.clone()
                    retry=Callback::new(move |()| reload.update(|n| *n += 1))
                />
            }
            .into_any();
        }
        if state == WallState::Loading {
            return view! {
                <div class="pick-wall pod-wall" role="status" aria-label="Loading picks">
                    {(0..8).map(|_| view! {
                        <div class="pick">
                            <Skeleton kind=SkeletonKind::Poster/>
                            <Skeleton width="70%"/>
                        </div>
                    }).collect_view()}
                </div>
            }
            .into_any();
        }
        if state == WallState::Empty {
            let message = if available.get_untracked() {
                "No podcast picks yet · Find picks to get the first one."
            } else {
                "No podcast picks yet · Add the recommendation configuration to enable runs."
            };
            return view! { <EmptyState message icon=Icon::Headphones/> }.into_any();
        }
        if state == WallState::NoMatch {
            return view! {
                <EmptyState
                    message="No picks with this status."
                    compact=true
                    action=ViewFn::from(move || view! {
                        <Button
                            size=ButtonSize::Sm
                            variant=ButtonVariant::Ghost
                            on_click=Callback::new(move |_| {
                                status_filter.set(None);
                                limit.set(PAGE);
                            })
                        >
                            "Show all"
                        </Button>
                    })
                />
            }
            .into_any();
        }
        view! {
            <div class="pick-wall pod-wall" aria-label="Podcast picks">
                <For
                    each=move || shown.get()
                    key=|rec| format!("{}|{rec:?}", rec.recommendation_id)
                    children=move |rec| {
                        let id = rec.recommendation_id.clone();
                        let saving_key = id.clone();
                        let open_id = id.clone();
                        let highlighted = highlight_id
                            .with_value(|h| h.as_deref() == Some(id.as_str()));
                        view! {
                            <PickCard
                                rec
                                saving=Signal::derive(move || {
                                    saving_id.with(|s| s.as_deref() == Some(saving_key.as_str()))
                                })
                                highlighted
                                on_open=Callback::new(move |()| selected.set(Some(open_id.clone())))
                                on_feedback=Callback::new(move |feedback| save(id.clone(), feedback))
                            />
                        }
                    }
                />
            </div>
            {move || (remaining.get() > 0).then(|| view! {
                <ShowMoreButton
                    remaining
                    noun="picks"
                    on_click=Callback::new(move |()| limit.update(|n| *n += PAGE))
                />
            })}
        }
        .into_any()
    };

    let inspector_saving =
        Signal::derive(move || saving_id.with(|s| s.is_some() && *s == selected.get()));

    view! {
        <PageHead title="Podcasts" lede=lede actions>
            <MediaSwitch/>
        </PageHead>

        {move || has_up_next.get().then(|| view! {
            <section class="section rec-deck" aria-label="Up next">
                <div class="section-head">
                    <h2 class="section-title">"Up next"</h2>
                    <span class="section-meta">"Newest picks, waiting for a listen"</span>
                </div>
                <div class="poster-rail pod-rail">
                    <For
                        each=move || up_next.get()
                        key=|rec| rec.recommendation_id.clone()
                        children=|rec| {
                            let duration = rec.duration_minutes.map(|m| format_minutes(m as f64));
                            view! {
                                <Link class="poster-card" to=pick_path(&rec.recommendation_id) title=rec.episode_title.clone()>
                                    <Poster src=rec.artwork_url.clone() title=rec.show_title.clone() square=true eager=true/>
                                    <span class="poster-card-title clamp-2">{rec.episode_title.clone()}</span>
                                    <span class="poster-card-why">
                                        {rec.show_title.clone()}
                                        {duration.map(|d| view! { <span class="num">{format!(" · {d}")}</span> })}
                                    </span>
                                </Link>
                            }
                        }
                    />
                </div>
            </section>
        })}

        <div class="split rec-layout">
            <section class="section rec-main" aria-label="Picks">
                <div class="section-head">
                    <h2 class="section-title">"Picks"</h2>
                </div>
                <div class="toolbar">
                    <div class="rec-filter">
                        <Segmented
                            options=filter_options
                            value=status_filter
                            on_change=Callback::new(move |status| {
                                status_filter.set(status);
                                limit.set(PAGE);
                            })
                            aria_label="Filter picks by status"
                            small=true
                        />
                    </div>
                </div>
                {wall}
            </section>
            <aside class="stack-lg rec-side">
                <TasteBrain
                    profile=taste
                    loading=taste_loading
                    error=taste_error
                    subtitle="What your listening and feedback say about your taste."
                    empty_text="No profile yet. The reflection task builds one from Castro listen history and pick feedback."
                    stats=taste_stats
                    collapsible=true
                />
                <Link to=format!("/operations#inspect={TASK_NAME}") class="textlink">
                    "Runs in Operations →"
                </Link>
            </aside>
        </div>

        {move || selected.with(Option::is_some).then(|| view! {
            <PickInspector
                rec=selected_rec
                saving=inspector_saving
                on_feedback=Callback::new(move |feedback| {
                    if let Some(id) = selected.get_untracked() {
                        save(id, feedback);
                    }
                })
                on_close=Callback::new(move |()| selected.set(None))
            />
        })}
    }
}

/// Copies `text` through `navigator.clipboard.writeText` (no web-sys
/// feature needed); `false` when the API is missing or refuses.
async fn copy_text(text: &str) -> bool {
    use wasm_bindgen::JsCast as _;
    let Ok(navigator) = js_sys::Reflect::get(&js_sys::global(), &"navigator".into()) else {
        return false;
    };
    let Ok(clipboard) = js_sys::Reflect::get(&navigator, &"clipboard".into()) else {
        return false;
    };
    let Ok(write) = js_sys::Reflect::get(&clipboard, &"writeText".into()) else {
        return false;
    };
    let Ok(write) = write.dyn_into::<js_sys::Function>() else {
        return false;
    };
    let Ok(promise) = write.call1(&clipboard, &text.into()) else {
        return false;
    };
    let Ok(promise) = promise.dyn_into::<js_sys::Promise>() else {
        return false;
    };
    wasm_bindgen_futures::JsFuture::from(promise).await.is_ok()
}

/// The feed URL in mono with a copy button.
#[component]
fn FeedUrl(url: String) -> impl IntoView {
    let copied = RwSignal::new(false);
    let toast = use_toast();
    let text = StoredValue::new(url.clone());
    let copy = move |_| {
        let text = text.get_value();
        spawn_detached(async move {
            if copy_text(&text).await {
                copied.try_set(true);
                sleep(Duration::from_secs(2)).await;
                copied.try_set(false);
            } else {
                toast.show("Could not copy the feed URL", ToastKind::Error);
            }
        });
    };
    view! {
        <div class="pod-feed">
            <a class="mono truncate" href=url.clone() target="_blank" rel="noreferrer" title=url.clone()>
                {url.clone()}
            </a>
            <Button
                size=ButtonSize::Sm
                variant=ButtonVariant::Ghost
                icon=Icon::Copy
                icon_only=true
                aria_label="Copy feed URL"
                title="Copy feed URL"
                on_click=Callback::new(copy)
            />
            <span class="small text-signal" role="status">{move || copied.get().then_some("Copied")}</span>
        </div>
    }
}

/// `/podcasts/:id`.
#[component]
pub fn PodcastDetailPage(#[prop(into)] id: String) -> impl IntoView {
    let rec = RwSignal::new(None::<PodcastRecommendation>);
    let error = RwSignal::new(None::<String>);
    let reload = RwSignal::new(0u32);
    let saving = RwSignal::new(false);
    let note_text = RwSignal::new(String::new());
    let saving_note = RwSignal::new(false);
    let toast = use_toast();

    use_page_label(move || rec.with(|r| r.as_ref().map(|r| r.episode_title.clone())));

    let load_id = id.clone();
    Effect::new(move |_| {
        reload.track();
        let id = load_id.clone();
        spawn_scoped(async move {
            match api::fetch_podcast_recommendation(&id).await {
                Ok(res) => {
                    note_text.set(res.recommendation.feedback_note.clone().unwrap_or_default());
                    rec.set(Some(res.recommendation));
                    error.set(None);
                }
                Err(err) => error.set(Some(err.message().to_owned())),
            }
        });
    });

    let save_id = StoredValue::new(id);
    let on_pick = Callback::new(move |feedback: PodcastFeedback| {
        if saving.get_untracked() {
            return;
        }
        saving.set(true);
        let id = save_id.get_value();
        spawn_detached(async move {
            match api::send_podcast_recommendation_feedback(&id, Some(feedback), None).await {
                Ok(result) => {
                    rec.set(Some(result.recommendation));
                    toast.show("Feedback saved", ToastKind::Info);
                }
                Err(err) => toast.show(err.message(), ToastKind::Error),
            }
            saving.set(false);
        });
    });
    let save_note = Callback::new(move |_| {
        saving_note.set(true);
        let id = save_id.get_value();
        let note = note_text.get_untracked().trim().to_owned();
        spawn_detached(async move {
            match api::send_podcast_recommendation_feedback(&id, None, Some(&note)).await {
                Ok(result) => {
                    rec.set(Some(result.recommendation));
                    toast.show("Note saved", ToastKind::Info);
                }
                Err(err) => toast.show(err.message(), ToastKind::Error),
            }
            saving_note.set(false);
        });
    });
    let current = Signal::derive(move || rec.with(|r| r.as_ref().and_then(|r| r.feedback)));
    let rec_id = Memo::new(move |_| rec.with(|r| r.as_ref().map(|r| r.recommendation_id.clone())));
    let rateable = Memo::new(move |_| rec.with(|r| r.as_ref().is_some_and(|r| can_rate(r.status))));

    // Rebuild the page only when a different pick loads; rating updates flow
    // through `current` and the timeline so focus and the note survive.
    move || {
        if let Some(message) = error.get() {
            return view! {
                <ErrorState
                    page=true
                    title="This pick could not load"
                    raw=message
                    retry=Callback::new(move |()| reload.update(|n| *n += 1))
                    link=("All podcast picks".to_owned(), "/podcasts".to_owned())
                />
            }
            .into_any();
        }
        if rec_id.get().is_none() {
            return view! {
                <div class="rec-detail pod-detail" role="status" aria-label="Loading pick">
                    <div class="rec-detail-art"><Skeleton kind=SkeletonKind::Poster/></div>
                    <div class="rec-detail-main stack">
                        <Skeleton width="30%"/>
                        <Skeleton kind=SkeletonKind::Title width="80%"/>
                        <Skeleton width="60%"/>
                    </div>
                </div>
            }
            .into_any();
        }
        let r = rec
            .get_untracked()
            .unwrap_or_else(|| unreachable!("loaded"));
        let why = r.why_for_user.clone().filter(|w| !w.is_empty());
        let scores = r.shortlist_scores.as_ref().map(score_rows);
        let duration = r.duration_minutes.map(|m| format_minutes(m as f64));
        view! {
            <div class="rec-detail pod-detail">
                <div class="rec-detail-art">
                    <Poster src=r.artwork_url.clone() title=r.show_title.clone() square=true eager=true/>
                </div>
                <div class="rec-detail-main">
                    <div class="cluster">
                        {move || rec.with(|r| r.as_ref().map(status_line))}
                        {duration.map(|d| view! { <Tag>{d}</Tag> })}
                        <Tag>{format!("Released {}", format_date_only(r.published_at as f64))}</Tag>
                    </div>
                    <h1 class="page-title rec-detail-title">{r.episode_title.clone()}</h1>
                    <p class="pod-detail-show">{show_line(&r)}</p>
                    <div class="cluster">{episode_links(&r, ButtonSize::Md)}</div>
                    {why.map(|why| view! { <p class="rec-why prose">{why}</p> })}
                    {(!r.caveats.is_empty()).then(|| view! {
                        <div class="rec-caveat-block">
                            <h2 class="label">"Before you listen"</h2>
                            {caveat_list(&r.caveats)}
                        </div>
                    })}
                    {move || rateable.get().then(|| view! {
                        <Panel title="Your take" pad=true class="rec-take">
                            <div class="rec-take-choices hide-phone">
                                <Choices
                                    choices=podcast_choices()
                                    current
                                    saving
                                    on_pick
                                    aria_label="Rate this pick"
                                />
                            </div>
                            <div class="field rec-note">
                                <label class="field-label" for="podrec-feedback-note">"Note"</label>
                                <textarea
                                    id="podrec-feedback-note"
                                    class="textarea"
                                    rows="3"
                                    placeholder="What worked, what didn't…"
                                    prop:value=move || note_text.get()
                                    on:input=move |event| note_text.set(event_target_value(&event))
                                    disabled=move || saving_note.get()
                                ></textarea>
                                <div class="cluster">
                                    <Button
                                        size=ButtonSize::Sm
                                        busy=saving_note
                                        disabled=Signal::derive(move || note_text.with(|t| t.trim().is_empty()))
                                        disabled_reason="Write a note first"
                                        on_click=save_note
                                    >
                                        "Save note"
                                    </Button>
                                </div>
                            </div>
                        </Panel>
                    })}
                    <div class="grid-2">
                        {scores.map(|scores| view! {
                            <Panel title="Shortlist scores" pad=true>{scores}</Panel>
                        })}
                        <Panel title="Timeline" pad=true>
                            {move || rec.with(|r| r.as_ref().map(timeline))}
                        </Panel>
                    </div>
                    <Panel title="Details" pad=true>
                        <div class="stack">
                            {fact_list(&r)}
                            <div class="field">
                                <span class="field-label">"Feed"</span>
                                <FeedUrl url=r.feed_url.clone()/>
                            </div>
                        </div>
                    </Panel>
                    {move || rateable.get().then(|| view! {
                        <div class="rec-take-bar only-phone">
                            <Choices
                                choices=podcast_choices()
                                current
                                saving
                                on_pick
                                aria_label="Rate this pick"
                            />
                        </div>
                    })}
                </div>
            </div>
        }
        .into_any()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(status: PodcastRecommendationStatus) -> PodcastRecommendation {
        PodcastRecommendation {
            recommendation_id: "r1".into(),
            show_title: "Show".into(),
            episode_title: "Episode".into(),
            feed_url: "https://example.com/feed".into(),
            itunes_id: None,
            artwork_url: None,
            episode_url: None,
            published_at: 0,
            duration_minutes: None,
            status,
            why_for_user: None,
            caveats: Vec::new(),
            confidence: None,
            shortlist_scores: None,
            discovered_via: None,
            source_url: None,
            matched_voices: Vec::new(),
            recommended_at: 0,
            notified_at: None,
            queue_result: None,
            feedback: None,
            feedback_at: None,
            feedback_note: None,
        }
    }

    #[test]
    fn queued_notified_picks_read_in_castro() {
        let mut queued = rec(PodcastRecommendationStatus::Notified);
        queued.queue_result = Some(PodcastQueueResult::AlreadyQueued);
        assert_eq!(pick_status(&queued), (StatusKind::Info, "In Castro"));
        assert_eq!(
            pick_status(&rec(PodcastRecommendationStatus::Notified)),
            (StatusKind::Idle, "Notified")
        );
        assert_eq!(
            pick_status(&rec(PodcastRecommendationStatus::Failed)).0,
            StatusKind::Fault
        );
    }

    #[test]
    fn lede_counts_the_castro_queue_and_listens() {
        let mut queued = rec(PodcastRecommendationStatus::Notified);
        queued.queue_result = Some(PodcastQueueResult::Queued);
        let list = vec![
            queued,
            rec(PodcastRecommendationStatus::Listened),
            rec(PodcastRecommendationStatus::Ignored),
        ];
        assert_eq!(podcasts_lede(&list), "1 waiting in Castro · 1 listened");
        assert_eq!(podcasts_lede(&list[2..]), "Nothing waiting in Castro");
    }

    #[test]
    fn deep_link_limit_reveals_the_target_page() {
        assert_eq!(limit_to_show(0), PAGE);
        assert_eq!(limit_to_show(PAGE - 1), PAGE);
        assert_eq!(limit_to_show(PAGE), 2 * PAGE);
    }

    #[test]
    fn only_settled_picks_take_feedback() {
        assert!(!can_rate(PodcastRecommendationStatus::Pending));
        assert!(!can_rate(PodcastRecommendationStatus::Failed));
        assert!(can_rate(PodcastRecommendationStatus::Notified));
    }
}
