//! Movies & TV: the picks wall with its inspector, and one pick's detail
//! page.

use std::time::Duration;

use leptos::prelude::*;
use omni_api::common::encode_uri_component;
use omni_api::media::{
    MediaType, Recommendation, RecommendationFeedback, RecommendationStatus, ShortlistScores,
    TasteProfile, WatchlistResult,
};
use omni_web_kit::api;
use omni_web_kit::components::{
    Button, ButtonLink, ButtonSize, ButtonVariant, EmptyState, ErrorState, Icon, Inspector, Meter,
    OnDeck, PageHead, Panel, Poster, RunButton, SegOption, Segmented, ShowMoreButton, Skeleton,
    SkeletonKind, Status, Tag, ToastKind, Tone, use_toast,
};
use omni_web_kit::hooks::{
    query_param, scroll_into_view_center, store_pref, stored_pref, use_is_phone,
};
use omni_web_kit::live::use_live_data;
use omni_web_kit::router::Link;
use omni_web_kit::task::{sleep, spawn_detached, spawn_scoped};
use omni_web_kit::utils::format::{format_absolute_with_year, format_date_only, format_relative};
use omni_web_kit::utils::js::{js_round, number_string, to_fixed};
use omni_web_kit::utils::rec_labels::{
    REC_FEEDBACK_ACTIONS, REC_STATUS_ORDER, rec_status_label, watchlist_label,
};

use crate::common::format_minutes;
use crate::rec_ui::{ChoiceStyle, Choices, MediaSwitch, media_choices, rec_status_kind};
use crate::taste_brain::{TasteBrain, TasteBrainProfile};

const TASK_NAME: &str = "Recommendations";
const TASTE_TASK_NAME: &str = "TasteReflection";
const MAX_PICKS: u32 = 10;
const PAGE: usize = 24;
const VIEW_PREF: &str = "omni.media-view";

fn poster_url(path: Option<&str>, size: &str) -> Option<String> {
    path.map(|p| format!("https://image.tmdb.org/t/p/{size}{p}"))
}

fn kind_label(media_type: MediaType) -> &'static str {
    if media_type == MediaType::Tv {
        "TV"
    } else {
        "Movie"
    }
}

fn year_text(rec: &Recommendation) -> Option<String> {
    rec.year.filter(|y| *y != 0.0).map(number_string)
}

/// Feedback opens once a pick has been delivered.
fn can_rate(status: RecommendationStatus) -> bool {
    status != RecommendationStatus::Pending && status != RecommendationStatus::Failed
}

fn watchlist_tone(result: WatchlistResult) -> Tone {
    match result {
        WatchlistResult::Error => Tone::Fault,
        WatchlistResult::Available => Tone::Ok,
        WatchlistResult::Added | WatchlistResult::AlreadyExists => Tone::Neutral,
    }
}

fn feedback_label(feedback: RecommendationFeedback) -> String {
    REC_FEEDBACK_ACTIONS
        .iter()
        .find(|(v, _)| *v == feedback)
        .map_or_else(|| feedback.as_str().to_owned(), |(_, l)| (*l).to_owned())
}

/// "3 new picks waiting · 41 watched" for the page lede.
pub fn picks_lede(recs: &[Recommendation]) -> String {
    let count = |status| recs.iter().filter(|r| r.status == status).count();
    let fresh = count(RecommendationStatus::Notified);
    let watched = count(RecommendationStatus::Watched);
    let mut parts = Vec::new();
    match fresh {
        0 => parts.push("No new picks waiting".to_owned()),
        1 => parts.push("1 new pick waiting".to_owned()),
        n => parts.push(format!("{n} new picks waiting")),
    }
    if watched > 0 {
        parts.push(format!("{watched} watched"));
    }
    parts.join(" · ")
}

/// The ids in display order for `filter` (`None` = every status).
pub fn filtered(
    recs: &[Recommendation],
    filter: Option<RecommendationStatus>,
) -> impl Iterator<Item = &Recommendation> {
    recs.iter()
        .filter(move |r| filter.is_none_or(|status| r.status == status))
}

/// How many cards to reveal so the pick at `index` is visible.
pub fn limit_to_show(index: usize) -> usize {
    (index / PAGE + 1) * PAGE
}

fn status_line(rec: &Recommendation) -> impl IntoView + use<> {
    let kind = rec_status_kind(rec.status);
    view! {
        <Status kind=kind label=rec_status_label(rec.status)/>
        {rec.watchlist_result.filter(|r| matches!(r, WatchlistResult::Available | WatchlistResult::Error)).map(|result| view! {
            <Tag tone=watchlist_tone(result)>{watchlist_label(result)}</Tag>
        })}
    }
}

fn caveat_list(items: &[String]) -> impl IntoView + use<> {
    view! {
        <ul class="rec-caveats">
            {items.iter().map(|c| view! { <li>{c.clone()}</li> }).collect_view()}
        </ul>
    }
}

fn score_rows(scores: &ShortlistScores) -> impl IntoView + use<> {
    let rows = [
        ("Taste match", scores.taste_match),
        ("Novelty", scores.novelty),
        ("Effort fit", scores.effort_fit),
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
                            <Meter value=value max=100.0 tone=tone label=format!("{label} score")/>
                            <span class="num">{number_string(js_round(value))}</span>
                        </div>
                    }
                })
                .collect_view()}
        </div>
        {risks}
    }
}

fn timeline(rec: &Recommendation) -> impl IntoView + use<> {
    let mut events: Vec<(String, f64)> = vec![("Recommended".to_owned(), rec.recommended_at)];
    if let Some(at) = rec.notified_at {
        events.push(("Notified".to_owned(), at));
    }
    if let Some(at) = rec.started_at {
        events.push(("Started watching".to_owned(), at));
    }
    if let Some(at) = rec.resolved_at {
        events.push((format!("Resolved · {}", rec_status_label(rec.status)), at));
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
                        <time class="num">{format_absolute_with_year(at)}</time>
                    </li>
                })
                .collect_view()}
        </ol>
    }
}

/// Facts as key-value rows; absent fields are skipped.
pub fn rec_facts(rec: &Recommendation) -> Vec<(&'static str, String)> {
    let join = |items: &[String]| items.join(", ");
    let mut fields = Vec::new();
    if !rec.genres.is_empty() {
        fields.push(("Genres", join(&rec.genres)));
    }
    if let Some(minutes) = rec.runtime_minutes {
        fields.push(("Runtime", format_minutes(minutes)));
    }
    if let Some(seasons) = rec.season_count {
        let episodes = rec
            .episode_count
            .map(|e| format!(" ({} episodes)", number_string(e)))
            .unwrap_or_default();
        fields.push(("Seasons", format!("{}{episodes}", number_string(seasons))));
    }
    if let Some(status) = rec.series_status.clone().filter(|s| !s.is_empty()) {
        fields.push(("Series status", status));
    }
    if let Some(rated) = rec.certification.clone().filter(|s| !s.is_empty()) {
        fields.push(("Rated", rated));
    }
    if let Some(language) = rec.original_language.as_ref().filter(|s| !s.is_empty()) {
        fields.push(("Language", language.to_uppercase()));
    }
    if !rec.origin_countries.is_empty() {
        fields.push(("Country", join(&rec.origin_countries)));
    }
    if !rec.creators.is_empty() {
        let label = if rec.media_type == MediaType::Movie {
            "Directed by"
        } else {
            "Created by"
        };
        fields.push((label, join(&rec.creators)));
    }
    if !rec.cast.is_empty() {
        fields.push(("Cast", join(&rec.cast)));
    }
    if !rec.keywords.is_empty() {
        fields.push(("Keywords", join(&rec.keywords)));
    }
    if let Some(source) = rec.source.clone().filter(|s| !s.is_empty()) {
        fields.push(("Source", source));
    }
    if let Some(confidence) = rec.confidence {
        fields.push((
            "Confidence",
            format!("{}%", number_string(js_round(confidence * 100.0))),
        ));
    }
    fields
}

fn fact_list(rec: &Recommendation) -> impl IntoView + use<> {
    view! {
        <dl class="kv rec-facts">
            {rec_facts(rec)
                .into_iter()
                .map(|(label, value)| view! { <dt>{label}</dt><dd>{value}</dd> })
                .collect_view()}
        </dl>
    }
}

fn manager_name(media_type: MediaType) -> &'static str {
    if media_type == MediaType::Movie {
        "Radarr"
    } else {
        "Sonarr"
    }
}

fn service_links(rec: &Recommendation, size: ButtonSize) -> impl IntoView + use<> {
    let manager = manager_name(rec.media_type);
    view! {
        <ButtonLink
            to=rec.links.plex.clone()
            external=true
            size
            aria_label=format!("Open {} in Plex", rec.title)
        >
            "Plex"
        </ButtonLink>
        <ButtonLink
            to=rec.links.manager.clone()
            external=true
            size
            aria_label=format!("Open {} in {manager}", rec.title)
        >
            {manager}
        </ButtonLink>
        <ButtonLink
            to=rec.links.tmdb.clone()
            external=true
            size
            variant=ButtonVariant::Ghost
            aria_label=format!("View {} on TMDB", rec.title)
        >
            "TMDB"
        </ButtonLink>
    }
}

/// Rating choices plus the optional note. `note_id` names the textarea on
/// the detail page (`#rec-feedback-note`); `bar_on_phone` hides these
/// choices on phones, where the page pins its own choice bar to the bottom.
#[component]
fn RecTake(
    #[prop(into)] current: Signal<Option<RecommendationFeedback>>,
    #[prop(into)] note: Signal<Option<String>>,
    #[prop(into)] saving: Signal<bool>,
    on_pick: Callback<RecommendationFeedback>,
    on_note: Callback<String>,
    #[prop(optional)] note_id: Option<&'static str>,
    #[prop(optional)] bar_on_phone: bool,
) -> impl IntoView {
    let text = RwSignal::new(note.get_untracked().unwrap_or_default());
    let blank = Signal::derive(move || text.with(|t| t.trim().is_empty()));
    view! {
        <div class=if bar_on_phone { "rec-take-choices hide-phone" } else { "rec-take-choices" }>
            <Choices
                choices=media_choices()
                current
                saving
                on_pick
                aria_label="Rate this pick"
            />
        </div>
        <div class="field rec-note">
            <label class="field-label" for=note_id>"Note"</label>
            <textarea
                id=note_id
                class="textarea"
                rows="3"
                aria-label=note_id.is_none().then_some("Note")
                placeholder="What worked, what didn't…"
                prop:value=move || text.get()
                on:input=move |event| text.set(event_target_value(&event))
                disabled=move || saving.get()
            ></textarea>
            <div class="cluster">
                <Button
                    size=ButtonSize::Sm
                    busy=saving
                    disabled=blank
                    disabled_reason="Write a note first"
                    on_click=Callback::new(move |_| on_note.run(text.get_untracked().trim().to_owned()))
                >
                    "Save note"
                </Button>
            </div>
        </div>
    }
}

/// A poster card on the picks wall. The poster and title open the inspector.
#[component]
fn PickCard(
    rec: Recommendation,
    #[prop(into)] saving: Signal<bool>,
    highlighted: bool,
    on_open: Callback<()>,
    on_feedback: Callback<RecommendationFeedback>,
) -> impl IntoView {
    let current = rec.feedback;
    let year = year_text(&rec);
    let why = rec.why_for_user.clone().filter(|w| !w.is_empty());
    view! {
        <article
            id=format!("recommendation-{}", rec.recommendation_id)
            class=if highlighted { "pick deep-link-target" } else { "pick" }
        >
            <button
                type="button"
                class="pick-hit"
                aria-label=format!("Details for {}", rec.title)
                on:click=move |_| on_open.run(())
            >
                <Poster
                    src=poster_url(rec.poster_path.as_deref(), "w342")
                    title=rec.title.clone()
                    kind=kind_label(rec.media_type)
                    captioned=true
                />
                <span class="pick-title">
                    {rec.title.clone()}
                    {year.map(|y| view! { <span class="num">{y}</span> })}
                </span>
            </button>
            <div class="pick-meta">{status_line(&rec)}</div>
            {why.map(|why| view! { <p class="pick-why">{why}</p> })}
            {can_rate(rec.status).then(|| view! {
                <Choices
                    choices=media_choices()
                    current=Signal::stored(current)
                    saving
                    on_pick=on_feedback
                    style=ChoiceStyle::Compact
                    aria_label=format!("Rate {}", rec.title)
                />
            })}
        </article>
    }
}

/// The selected pick in a drawer (bottom sheet on phone).
#[component]
fn PickInspector(
    #[prop(into)] rec: Signal<Option<Recommendation>>,
    #[prop(into)] saving: Signal<bool>,
    on_feedback: Callback<RecommendationFeedback>,
    on_note: Callback<String>,
    on_close: Callback<()>,
) -> impl IntoView {
    let title = Signal::derive(move || {
        rec.with(|r| {
            r.as_ref().map_or_else(String::new, |r| match year_text(r) {
                Some(y) => format!("{} ({y})", r.title),
                None => r.title.clone(),
            })
        })
    });
    let status = ViewFn::from(move || {
        move || {
            rec.with(|r| {
                r.as_ref().map(|r| {
                    let kind = kind_label(r.media_type);
                    view! {
                        <Tag>{kind}</Tag>
                        {status_line(r)}
                    }
                })
            })
        }
    });
    let actions = ViewFn::from(move || {
        move || {
            rec.with(|r| {
                r.as_ref().map(|r| {
                    let page = format!("/media/{}", encode_uri_component(&r.recommendation_id));
                    view! {
                        {service_links(r, ButtonSize::Sm)}
                        <ButtonLink to=page size=ButtonSize::Sm variant=ButtonVariant::Ghost>
                            "Open page"
                        </ButtonLink>
                    }
                })
            })
        }
    });
    let rec_id = Memo::new(move |_| rec.with(|r| r.as_ref().map(|r| r.recommendation_id.clone())));
    let current = Signal::derive(move || rec.with(|r| r.as_ref().and_then(|r| r.feedback)));
    let note =
        Signal::derive(move || rec.with(|r| r.as_ref().and_then(|r| r.feedback_note.clone())));
    let rateable = Memo::new(move |_| rec.with(|r| r.as_ref().is_some_and(|r| can_rate(r.status))));
    view! {
        <Inspector title status actions on_close>
            {move || {
                // Rebuilt only when another pick is selected, so a note being
                // typed survives a rating save.
                rec_id.track();
                rec.get_untracked().map(|r| {
                    let why = r.why_for_user.clone().filter(|w| !w.is_empty());
                    let facts = rec_facts(&r);
                    view! {
                        <div class="rec-inspector-art">
                            <Poster
                                src=poster_url(r.poster_path.as_deref(), "w342")
                                title=r.title.clone()
                                eager=true
                            />
                            {why.map(|why| view! { <p class="rec-why">{why}</p> })}
                        </div>
                        {(!r.caveats.is_empty()).then(|| view! {
                            <section class="inspector-section">
                                <h3>"Before you watch"</h3>
                                {caveat_list(&r.caveats)}
                            </section>
                        })}
                        {move || rateable.get().then(|| view! {
                            <section class="inspector-section rec-take">
                                <h3>"Your take"</h3>
                                <RecTake current note saving on_pick=on_feedback on_note/>
                            </section>
                        })}
                        {r.shortlist_scores.as_ref().map(|s| view! {
                            <section class="inspector-section">
                                <h3>"Shortlist scores"</h3>
                                {score_rows(s)}
                            </section>
                        })}
                        {(!facts.is_empty()).then(|| view! {
                            <section class="inspector-section">
                                <h3>"Details"</h3>
                                {fact_list(&r)}
                            </section>
                        })}
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

fn media_stats(profile: &TasteProfile) -> Vec<(String, String)> {
    let stats = &profile.stats;
    vec![
        ("Movies finished".into(), stats.completed_movies.to_string()),
        ("Series finished".into(), stats.completed_series.to_string()),
        ("Rewatched".into(), stats.rewatched_titles.to_string()),
        (
            "Picks watched".into(),
            format!(
                "{}/{}",
                stats.recommendations.watched, stats.recommendations.total
            ),
        ),
        ("Good picks".into(), stats.feedback.good_pick.to_string()),
        (
            "Time to start".into(),
            match stats.average_hours_to_start {
                None => "–".into(),
                Some(hours) => format!("{}h", to_fixed(hours, 1)),
            },
        ),
    ]
}

/// Grid of posters, or (phone only) a list with thumbnails.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WallView {
    Grid,
    List,
}

/// What the picks wall shows; the wall re-renders only when this changes.
#[derive(Clone, Debug, PartialEq, Eq)]
enum WallState {
    Loading,
    Error(String),
    Empty,
    NoMatch,
    List,
}

/// `/media` (and `/recommendations`).
#[component]
pub fn MediaPage() -> impl IntoView {
    let recs = RwSignal::new(None::<Vec<Recommendation>>);
    let recs_error = RwSignal::new(None::<String>);
    let reload = RwSignal::new(0u32);
    let status_filter = RwSignal::new(None::<RecommendationStatus>);
    let limit = RwSignal::new(PAGE);
    let saving_id = RwSignal::new(None::<String>);
    let selected = RwSignal::new(None::<String>);
    let picks = RwSignal::new(1u32);
    let taste_profile = RwSignal::new(None::<TasteProfile>);
    let taste_loading = RwSignal::new(true);
    let taste_error = RwSignal::new(None::<String>);
    let wall_view = RwSignal::new(match stored_pref(VIEW_PREF).as_deref() {
        Some("list") => WallView::List,
        _ => WallView::Grid,
    });
    let phone = use_is_phone();
    let live = use_live_data();
    let toast = use_toast();

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
    // Once the snapshot has loaded, a missing Recommendations task means it's
    // disabled server-side (missing API keys): no doomed Run button.
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

    // Load once, then reload whenever the task finishes running so fresh
    // picks appear without a manual refresh.
    Effect::new(move |_| {
        reload.track();
        if running.get() {
            return;
        }
        spawn_scoped(async move {
            match api::fetch_recommendations().await {
                Ok(data) => {
                    recs.set(Some(data.recommendations));
                    recs_error.set(None);
                }
                Err(err) => recs_error.set(Some(err.message().to_owned())),
            }
        });
    });

    Effect::new(move |_| {
        latest_taste_run_id.track();
        spawn_scoped(async move {
            match api::fetch_taste_profile().await {
                Ok(data) => {
                    taste_profile.set(data.profile);
                    taste_error.set(None);
                }
                Err(err) => taste_error.set(Some(err.message().to_owned())),
            }
            taste_loading.set(false);
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
                list.as_deref()
                    .map(|list| {
                        filtered(list, Some(status))
                            .position(|r| r.recommendation_id == id)
                            .unwrap_or(0)
                    })
                    .unwrap_or(0)
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

    let save = move |id: String, feedback: Option<RecommendationFeedback>, note: Option<String>| {
        saving_id.set(Some(id.clone()));
        spawn_detached(async move {
            match api::send_recommendation_feedback(&id, feedback, note.as_deref()).await {
                Ok(result) => {
                    recs.update(|list| {
                        if let Some(list) = list {
                            for rec in list.iter_mut() {
                                if rec.recommendation_id == id {
                                    *rec = result.recommendation.clone();
                                }
                            }
                        }
                    });
                    let message = if feedback.is_some() {
                        "Feedback saved"
                    } else {
                        "Note saved"
                    };
                    toast.show(message, ToastKind::Info);
                }
                Err(err) => toast.show(err.message(), ToastKind::Error),
            }
            saving_id.set(None);
        });
    };

    let counts = Memo::new(move |_| {
        recs.with(|list| {
            let list = list.as_deref().unwrap_or_default();
            REC_STATUS_ORDER
                .iter()
                .map(|s| (*s, list.iter().filter(|r| r.status == *s).count()))
                .collect::<Vec<_>>()
        })
    });
    let total = Memo::new(move |_| recs.with(|r| r.as_ref().map_or(0, Vec::len)));
    let filter_options = Signal::derive(move || {
        let mut options = vec![SegOption::new(None, "All").with_count(total.get())];
        options.extend(counts.get().into_iter().map(|(status, n)| {
            SegOption::new(Some(status), rec_status_label(status)).with_count(n)
        }));
        options
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
    let selected_rec = Signal::derive(move || {
        let id = selected.get()?;
        recs.with(|list| {
            list.as_ref()?
                .iter()
                .find(|r| r.recommendation_id == id)
                .cloned()
        })
    });
    // Memos: every snapshot frame would otherwise rebuild the rail and reset
    // its scroll position.
    let on_deck = Memo::new(move |_| {
        live.snapshot
            .with(|s| s.as_ref().map(|s| s.on_deck.clone()).unwrap_or_default())
    });
    let has_on_deck = Memo::new(move |_| on_deck.with(|items| !items.is_empty()));
    let lede = Signal::derive(move || recs.with(|r| r.as_deref().map(picks_lede)));

    let taste =
        Signal::derive(move || taste_profile.with(|p| p.as_ref().map(TasteBrainProfile::from)));
    let taste_stats = Signal::derive(move || {
        taste_profile.with(|p| p.as_ref().map(media_stats).unwrap_or_default())
    });
    // Run inside the taste body, which re-renders when the profile changes.
    let footer = ViewFn::from(move || {
        taste_profile.with_untracked(|p| {
            p.as_ref().map(|p| {
                let c = &p.commitment_preferences;
                let fit = |label: &'static str, pref: &str| {
                    view! { <dt>{label}</dt><dd>{pref.to_owned()}</dd> }
                };
                view! {
                    <div class="taste-group">
                        <h4 class="label">"Commitment fit"</h4>
                        <dl class="kv">
                            {fit("Movies", c.movies.preference.as_str())}
                            {fit("Limited series", c.limited_series.preference.as_str())}
                            {fit("Long series", c.long_series.preference.as_str())}
                        </dl>
                    </div>
                }
            })
        })
    });

    let picks_select = (1..=MAX_PICKS)
        .map(|n| view! { <option value=n.to_string() prop:selected=move || picks.get() == n>{n}</option> })
        .collect_view();
    let actions = ViewFn::from(move || {
        let picks_select = picks_select.clone();
        move || {
            if available.get() {
                view! {
                <label class="picks-count">
                    <span class="label">"Picks"</span>
                    <select
                        class="select"
                        aria-label="Maximum recommendations"
                        disabled=move || running.get()
                        on:change=move |event| picks.set(event_target_value(&event).parse().unwrap_or(1))
                    >
                        {picks_select.clone()}
                    </select>
                </label>
                <RunButton
                    task=TASK_NAME
                    running
                    label="Run picks"
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
                        disabled_reason="Task disabled: missing TMDB/OpenAI/Tavily API keys"
                    >
                        "Run picks"
                    </Button>
                    <span class="small muted only-phone">
                        "Runs are off: the TMDB, OpenAI and Tavily keys are missing."
                    </span>
                }
                .into_any()
            }
        }
    });

    let view_options = vec![
        SegOption::new(WallView::Grid, "Grid"),
        SegOption::new(WallView::List, "List"),
    ];
    let wall_class = move || {
        if phone.get() && wall_view.get() == WallView::List {
            "pick-wall list"
        } else {
            "pick-wall"
        }
    };
    let highlight_id = StoredValue::new(highlight);
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

    let wall = move || {
        let state = wall_state.get();
        if let WallState::Error(error) = &state {
            return view! {
                <ErrorState
                    title="Picks could not load"
                    raw=error.clone()
                    retry=Callback::new(move |()| reload.update(|n| *n += 1))
                />
            }
            .into_any();
        }
        if state == WallState::Loading {
            return view! {
                <div class="pick-wall" role="status" aria-label="Loading picks">
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
                "No picks yet · Run picks to get the first one."
            } else {
                "No picks yet · Add the recommendation service credentials to enable runs."
            };
            return view! { <EmptyState message icon=Icon::Film/> }.into_any();
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
            <div class=wall_class aria-label="Recommendations">
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
                                on_feedback=Callback::new(move |feedback| {
                                    save(id.clone(), Some(feedback), None)
                                })
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
        <PageHead title="Movies & TV" lede=lede actions>
            <MediaSwitch/>
        </PageHead>

        {move || has_on_deck.get().then(|| view! {
            <section class="section rec-deck" aria-label="On deck">
                <div class="section-head">
                    <h2 class="section-title">"On deck"</h2>
                    <span class="section-meta">"Ready in Plex, picked for you"</span>
                </div>
                <OnDeck items=on_deck/>
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
                    <div class="only-phone rec-view-toggle">
                        <Segmented
                            options=view_options
                            value=wall_view
                            on_change=Callback::new(move |view| {
                                wall_view.set(view);
                                store_pref(VIEW_PREF, if view == WallView::List { "list" } else { "grid" });
                            })
                            aria_label="Picks layout"
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
                    subtitle="What your watching and feedback say about your taste."
                    empty_text="No profile yet. The reflection task builds one from Plex watching and pick feedback."
                    stats=taste_stats
                    footer=footer
                    collapsible=true
                />
                <Link to="/operations#inspect=Recommendations" class="textlink">
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
                        save(id, Some(feedback), None);
                    }
                })
                on_note=Callback::new(move |note: String| {
                    if let Some(id) = selected.get_untracked() {
                        save(id, None, Some(note));
                    }
                })
                on_close=Callback::new(move |()| selected.set(None))
            />
        })}
    }
}

/// `/media/:id`.
#[component]
pub fn MediaDetailPage(#[prop(into)] id: String) -> impl IntoView {
    let rec = RwSignal::new(None::<Recommendation>);
    let error = RwSignal::new(None::<String>);
    let reload = RwSignal::new(0u32);
    let saving = RwSignal::new(false);
    let toast = use_toast();

    omni_web_kit::chrome::use_page_label(move || rec.with(|r| r.as_ref().map(|r| r.title.clone())));

    let load_id = id.clone();
    Effect::new(move |_| {
        reload.track();
        let id = load_id.clone();
        spawn_scoped(async move {
            match api::fetch_recommendation(&id).await {
                Ok(res) => {
                    rec.set(Some(res.recommendation));
                    error.set(None);
                }
                Err(err) => error.set(Some(err.message().to_owned())),
            }
        });
    });

    let save_id = StoredValue::new(id);
    let save = move |feedback: Option<RecommendationFeedback>, note: Option<String>| {
        saving.set(true);
        let id = save_id.get_value();
        spawn_detached(async move {
            match api::send_recommendation_feedback(&id, feedback, note.as_deref()).await {
                Ok(result) => {
                    rec.set(Some(result.recommendation));
                    let message = if feedback.is_some() {
                        "Feedback saved"
                    } else {
                        "Note saved"
                    };
                    toast.show(message, ToastKind::Info);
                }
                Err(err) => toast.show(err.message(), ToastKind::Error),
            }
            saving.set(false);
        });
    };
    let on_pick = Callback::new(move |feedback| save(Some(feedback), None));
    let on_note = Callback::new(move |note: String| save(None, Some(note)));
    let current = Signal::derive(move || rec.with(|r| r.as_ref().and_then(|r| r.feedback)));
    let note =
        Signal::derive(move || rec.with(|r| r.as_ref().and_then(|r| r.feedback_note.clone())));

    // Rebuild the page only when a different pick loads; rating updates
    // flow through `current`/`note` so the note being typed survives.
    let loaded_id =
        Memo::new(move |_| rec.with(|r| r.as_ref().map(|r| r.recommendation_id.clone())));
    let rateable = Memo::new(move |_| rec.with(|r| r.as_ref().is_some_and(|r| can_rate(r.status))));

    move || {
        if let Some(err) = error.get().filter(|_| rec.with(Option::is_none)) {
            return view! {
                <ErrorState
                    title="This pick could not load"
                    raw=err
                    retry=Callback::new(move |()| reload.update(|n| *n += 1))
                    link=("All picks".to_owned(), "/media".to_owned())
                    page=true
                />
            }
            .into_any();
        }
        if loaded_id.get().is_none() {
            return view! {
                <div class="rec-detail" role="status" aria-label="Loading pick">
                    <div class="rec-detail-art"><Skeleton kind=SkeletonKind::Poster/></div>
                    <div class="rec-detail-main stack">
                        <Skeleton width="30%"/>
                        <Skeleton kind=SkeletonKind::Title/>
                        <Skeleton width="80%"/>
                        <Skeleton width="65%"/>
                    </div>
                </div>
            }
            .into_any();
        }
        let r = rec
            .get_untracked()
            .unwrap_or_else(|| unreachable!("loaded"));
        let why = r.why_for_user.clone().filter(|w| !w.is_empty());
        let facts = (!rec_facts(&r).is_empty()).then(|| fact_list(&r));
        let scores = r.shortlist_scores.as_ref().map(score_rows);
        let kind = kind_label(r.media_type);
        view! {
            <div class="rec-detail">
                <div class="rec-detail-art">
                    <Poster
                        src=poster_url(r.poster_path.as_deref(), "w500")
                        title=r.title.clone()
                        eager=true
                        captioned=true
                    />
                </div>
                <div class="rec-detail-main">
                    <div class="cluster">
                        <Tag>{kind}</Tag>
                        {year_text(&r).map(|y| view! { <Tag>{y}</Tag> })}
                        {move || rec.with(|r| r.as_ref().map(status_line))}
                    </div>
                    <h1 class="page-title rec-detail-title">{r.title.clone()}</h1>
                    <p class="small muted">
                        {format!("Picked {} · {}", format_date_only(r.recommended_at), format_relative(r.recommended_at))}
                    </p>
                    <div class="cluster">{service_links(&r, ButtonSize::Md)}</div>
                    {why.map(|why| view! { <p class="rec-why prose">{why}</p> })}
                    {(!r.caveats.is_empty()).then(|| view! {
                        <div class="rec-caveat-block">
                            <h2 class="label">"Before you watch"</h2>
                            {caveat_list(&r.caveats)}
                        </div>
                    })}
                    {move || rateable.get().then(|| view! {
                        <Panel title="Your take" pad=true class="rec-take">
                            <RecTake
                                current
                                note
                                saving
                                on_pick
                                on_note
                                note_id="rec-feedback-note"
                                bar_on_phone=true
                            />
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
                    {facts.map(|facts| view! {
                        <Panel title="Details" pad=true>{facts}</Panel>
                    })}
                    {move || rateable.get().then(|| view! {
                        <div class="rec-take-bar only-phone">
                            <Choices
                                choices=media_choices()
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

    fn rec(id: &str, status: RecommendationStatus) -> Recommendation {
        serde_json::from_value(serde_json::json!({
            "recommendationId": id,
            "canonicalId": id,
            "tmdbId": 1,
            "mediaType": "movie",
            "title": id,
            "year": null,
            "posterPath": null,
            "status": status.as_str(),
            "whyForUser": null,
            "caveats": [],
            "runDate": "2026-10-01",
            "recommendedAt": 1,
            "notifiedAt": null,
            "startedAt": null,
            "resolvedAt": null,
            "watchlistResult": null,
            "confidence": null,
            "feedback": null,
            "feedbackAt": null,
            "feedbackNote": null,
            "source": null,
            "genres": [],
            "runtimeMinutes": null,
            "seasonCount": null,
            "episodeCount": null,
            "seriesStatus": null,
            "originalLanguage": null,
            "originCountries": [],
            "creators": [],
            "cast": [],
            "keywords": [],
            "certification": null,
            "shortlistScores": null,
            "links": {"tmdb": "t", "plex": "p", "manager": "m"}
        }))
        .expect("recommendation fixture")
    }

    #[test]
    fn lede_counts_new_and_watched_picks() {
        let list = [
            rec("a", RecommendationStatus::Notified),
            rec("b", RecommendationStatus::Notified),
            rec("c", RecommendationStatus::Watched),
            rec("d", RecommendationStatus::Ignored),
        ];
        assert_eq!(picks_lede(&list), "2 new picks waiting · 1 watched");
        assert_eq!(picks_lede(&list[3..]), "No new picks waiting");
    }

    #[test]
    fn filter_keeps_order_and_none_means_all() {
        let list = [
            rec("a", RecommendationStatus::Watched),
            rec("b", RecommendationStatus::Notified),
            rec("c", RecommendationStatus::Watched),
        ];
        let ids = |f| {
            filtered(&list, f)
                .map(|r| r.recommendation_id.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(None), ["a", "b", "c"]);
        assert_eq!(ids(Some(RecommendationStatus::Watched)), ["a", "c"]);
    }

    #[test]
    fn deep_link_reveals_enough_pages() {
        assert_eq!(limit_to_show(0), PAGE);
        assert_eq!(limit_to_show(PAGE - 1), PAGE);
        assert_eq!(limit_to_show(PAGE), PAGE * 2);
    }

    #[test]
    fn facts_skip_missing_fields() {
        let mut r = rec("a", RecommendationStatus::Watched);
        assert!(rec_facts(&r).is_empty());
        r.runtime_minutes = Some(125.0);
        r.creators = vec!["Ann".into()];
        assert_eq!(
            rec_facts(&r),
            vec![
                ("Runtime", "2h 5m".to_owned()),
                ("Directed by", "Ann".to_owned())
            ]
        );
    }
}
