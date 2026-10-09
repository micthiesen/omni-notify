//! Media recommendations and one pick's
//! detail page.

use std::collections::HashMap;

use leptos::prelude::*;
use omni_api::common::encode_uri_component;
use omni_api::media::{
    MediaType, Recommendation, RecommendationFeedback, RecommendationStatus, TasteProfile,
};
use omni_web_kit::api;
use omni_web_kit::components::{
    ImageWithFallback, OnDeck, ShowMoreButton, StatusFilterChips, Toast, ToastKind, use_show_more,
    use_toast,
};
use omni_web_kit::hooks::use_rec_highlight;
use omni_web_kit::live::use_live_data;
use omni_web_kit::router::Link;
use omni_web_kit::task::{spawn_detached, spawn_scoped};
use omni_web_kit::utils::format::format_date_only;
use omni_web_kit::utils::js::{js_round, number_string, to_fixed};
use omni_web_kit::utils::rec_labels::{
    REC_FEEDBACK_ACTIONS, REC_STATUS_ORDER, rec_status_label, watchlist_label,
};

use crate::common::{
    BackLink, DetailField, RunControls, ScoreRow, TimelineRow, active_if, format_minutes,
};
use crate::recommendation_runs::RecommendationRuns;
use crate::taste_brain::{TasteBrain, TasteBrainProfile};

const TASK_NAME: &str = "Recommendations";
const TASTE_TASK_NAME: &str = "TasteReflection";

fn media_badge(media_type: MediaType) -> impl IntoView {
    view! {
        <span class=format!("media-badge media-{}", media_type.as_str())>
            {if media_type == MediaType::Tv { "TV" } else { "Movie" }}
        </span>
    }
}

fn can_rate(status: RecommendationStatus) -> bool {
    status != RecommendationStatus::Pending && status != RecommendationStatus::Failed
}

fn title_with_year(rec: &Recommendation) -> impl IntoView + use<> {
    let year = rec.year.map(|year| {
        view! { <span class="rec-year">{format!(" ({})", number_string(year))}</span> }
    });
    view! {
        {rec.title.clone()}
        {year}
    }
}

fn status_badges(rec: &Recommendation) -> impl IntoView + use<> {
    view! {
        {media_badge(rec.media_type)}
        <span class=format!("status-chip status-chip-{}", rec.status.as_str())>
            {rec_status_label(rec.status)}
        </span>
        {rec
            .watchlist_result
            .map(|result| {
                view! {
                    <span class=format!("watchlist-badge watchlist-{}", result.as_str())>
                        {watchlist_label(result)}
                    </span>
                }
            })}
    }
}

/// The rating buttons; `current` is the stored feedback.
fn feedback_buttons(
    current: Option<RecommendationFeedback>,
    saving: Signal<bool>,
    on_feedback: Callback<RecommendationFeedback>,
) -> impl IntoView {
    view! {
        <div class="rec-feedback" aria-label="Recommendation feedback">
            {REC_FEEDBACK_ACTIONS
                .iter()
                .map(|(value, label)| {
                    let value = *value;
                    let pressed = current == Some(value);
                    view! {
                        <button
                            type="button"
                            class=format!("feedback-btn {}", active_if(pressed))
                            aria-pressed=pressed.to_string()
                            disabled=move || saving.get()
                            on:click=move |_| on_feedback.run(value)
                        >
                            {*label}
                        </button>
                    }
                })
                .collect_view()}
        </div>
    }
}

#[component]
fn RecommendationCard(
    rec: Recommendation,
    #[prop(into)] saving: Signal<bool>,
    highlighted: bool,
    on_feedback: Callback<RecommendationFeedback>,
) -> impl IntoView {
    let detail_path = format!("/media/{}", encode_uri_component(&rec.recommendation_id));
    let poster = rec
        .poster_path
        .as_ref()
        .map(|p| format!("https://image.tmdb.org/t/p/w185{p}"));
    let caveats = (!rec.caveats.is_empty()).then(|| {
        view! {
            <details class="content-disclosure rec-caveat-disclosure">
                <summary>"Before You Watch"</summary>
                <ul class="rec-caveats">
                    {rec.caveats.iter().map(|c| view! { <li>{c.clone()}</li> }).collect_view()}
                </ul>
            </details>
        }
    });
    let title = title_with_year(&rec);
    view! {
        <div
            id=format!("recommendation-{}", rec.recommendation_id)
            class=format!("rec-card {}", if highlighted { "rec-card-highlighted" } else { "" })
        >
            <ImageWithFallback
                src=poster
                alt=format!("{} poster", rec.title)
                class="rec-poster"
                placeholder_class="rec-poster-placeholder"
                lazy=true
                placeholder=|| {
                    view! {
                        <svg
                            width="28"
                            height="28"
                            viewBox="0 0 24 24"
                            fill="none"
                            stroke="currentColor"
                            stroke-width="1.5"
                            stroke-linecap="round"
                            stroke-linejoin="round"
                        >
                            <rect x="3" y="4" width="18" height="16" rx="2"></rect>
                            <path d="M3 9h18M7 4v5M12 4v5M17 4v5"></path>
                        </svg>
                    }
                }
            />
            <div class="rec-body">
                <div class="rec-title-row">
                    <Link to=detail_path.clone() class="rec-title rec-title-link" title="View details">
                        {title}
                    </Link>
                </div>
                <div class="rec-badges">{status_badges(&rec)}</div>
                {rec.why_for_user.clone().filter(|w| !w.is_empty()).map(|why| view! { <p class="rec-why">{why}</p> })}
                {caveats}
                <div class="rec-meta meta-row">
                    <span>{format!("Recommended {}", format_date_only(rec.recommended_at))}</span>
                </div>
                <div class="rec-links">
                    <a
                        class="content-primary-link"
                        href=rec.links.plex.clone()
                        target="_blank"
                        rel="noreferrer"
                    >
                        "Open in Plex "
                        <span aria-hidden="true">"↗"</span>
                    </a>
                    <Link to=detail_path>"Details " <span aria-hidden="true">"→"</span></Link>
                </div>
                {can_rate(rec.status).then(|| feedback_buttons(rec.feedback, saving, on_feedback))}
            </div>
        </div>
    }
}

fn media_stats(profile: &TasteProfile) -> Vec<(String, String)> {
    let stats = &profile.stats;
    vec![
        (
            "Completed Movies".into(),
            stats.completed_movies.to_string(),
        ),
        (
            "Completed Series".into(),
            stats.completed_series.to_string(),
        ),
        (
            "Rewatched Titles".into(),
            stats.rewatched_titles.to_string(),
        ),
        (
            "Recommendations Watched".into(),
            format!(
                "{}/{}",
                stats.recommendations.watched, stats.recommendations.total
            ),
        ),
        ("Good Picks".into(), stats.feedback.good_pick.to_string()),
        (
            "Average Time to Start".into(),
            match stats.average_hours_to_start {
                None => "Not enough data".into(),
                Some(hours) => format!("{}h", to_fixed(hours, 1)),
            },
        ),
    ]
}

/// The Watch page: run controls, On Deck, taste brain, filterable picks.
#[component]
pub fn MediaPage() -> impl IntoView {
    let recs = RwSignal::new(None::<Vec<Recommendation>>);
    let recs_error = RwSignal::new(None::<String>);
    let status_filter = RwSignal::new(String::new());
    let saving_id = RwSignal::new(None::<String>);
    let taste_profile = RwSignal::new(None::<TasteProfile>);
    let taste_loading = RwSignal::new(true);
    let taste_error = RwSignal::new(None::<String>);
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
    let available = Signal::derive(move || match task_found.get() {
        None => true,
        Some(found) => found.is_some(),
    });
    let latest_run = move |task: &'static str| {
        Memo::new(move |_| {
            live.snapshot.with(|s| {
                s.as_ref()
                    .and_then(|s| s.runs.iter().find(|r| r.task_name == task))
                    .map(|r| r.run_id.clone())
            })
        })
    };
    let latest_taste_run_id = latest_run(TASTE_TASK_NAME);
    let latest_recommendation_run_id = latest_run(TASK_NAME);

    // Load once, then reload whenever the task finishes running so fresh
    // picks appear without a manual refresh.
    Effect::new(move |_| {
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
                    taste_loading.set(false);
                }
                Err(err) => {
                    taste_error.set(Some(err.message().to_owned()));
                    taste_loading.set(false);
                }
            }
        });
    });

    let on_feedback = move |recommendation_id: String, feedback: RecommendationFeedback| {
        saving_id.set(Some(recommendation_id.clone()));
        spawn_detached(async move {
            match api::send_recommendation_feedback(&recommendation_id, Some(feedback), None).await
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

    let highlighted_id = use_rec_highlight(Signal::derive(move || recs.with(Option::is_some)));

    let status_counts = Memo::new(move |_| {
        let mut counts = HashMap::<String, usize>::new();
        recs.with(|list| {
            for rec in list.iter().flatten() {
                *counts.entry(rec.status.as_str().to_owned()).or_default() += 1;
            }
        });
        counts
    });
    let visible = Memo::new(move |_| {
        let filter = status_filter.get();
        recs.with(|list| {
            list.as_ref().map(|list| {
                list.iter()
                    .filter(|r| filter.is_empty() || r.status.as_str() == filter)
                    .cloned()
                    .collect::<Vec<_>>()
            })
        })
    });
    let show = use_show_more(
        Signal::derive(move || visible.get().unwrap_or_default()),
        20,
        status_filter.into(),
    );

    let taste =
        Signal::derive(move || taste_profile.with(|p| p.as_ref().map(TasteBrainProfile::from)));
    let taste_stats = Signal::derive(move || {
        taste_profile.with(|p| p.as_ref().map(media_stats).unwrap_or_default())
    });
    let footer = ViewFn::from(move || {
        taste_profile.with(|p| {
            p.as_ref().map(|p| {
                let c = &p.commitment_preferences;
                view! {
                    <div class="taste-commitments">
                        <span>"Commitment Fit"</span>
                        <span>{format!("Movies: {}", c.movies.preference.as_str())}</span>
                        <span>{format!("Limited Series: {}", c.limited_series.preference.as_str())}</span>
                        <span>{format!("Long Series: {}", c.long_series.preference.as_str())}</span>
                    </div>
                }
            })
        })
    });

    let order: Vec<(String, String)> = REC_STATUS_ORDER
        .iter()
        .map(|s| (s.as_str().to_owned(), rec_status_label(*s).to_owned()))
        .collect();
    let on_deck = Signal::derive(move || {
        live.snapshot
            .with(|s| s.as_ref().map(|s| s.on_deck.clone()).unwrap_or_default())
    });
    let highlight = highlighted_id.clone();

    view! {
        <div class="page-header">
            <div class="page-header-stack">
                <h1>"Watch"</h1>
                <p class="page-subtitle">"Films and series picked for your next night in."</p>
            </div>
            <RunControls
                task_name=TASK_NAME
                select_label="Maximum recommendations"
                max_options=10
                disabled_title="Task disabled: missing TMDB/OpenAI/Tavily API keys"
                running=running
                available=available
                live=live
                toast=toast
            />
        </div>
        <Toast toast=toast.toast />

        <OnDeck items=on_deck />

        <TasteBrain
            profile=taste
            loading=taste_loading
            error=taste_error
            subtitle="What your watching and feedback say about your taste."
            empty_text="No profile yet. The reflection task will build one from Plex watching and recommendation feedback."
            stats=taste_stats
            footer=footer
            collapsible=true
        />

        {move || {
            let total = recs.with(|r| r.as_ref().map_or(0, Vec::len));
            (total > 0)
                .then(|| {
                    view! {
                        <StatusFilterChips
                            order=order.clone()
                            counts=status_counts
                            total=total
                            active=status_filter
                            on_change=Callback::new(move |key: String| status_filter.set(key))
                        />
                    }
                })
        }}

        {move || {
            (recs.with(Option::is_none) && recs_error.with(Option::is_none))
                .then(|| view! { <div class="loading">"Loading…"</div> })
        }}
        {move || {
            recs_error
                .get()
                .map(|error| {
                    view! {
                        <div class="error">
                            <div>"Failed to load recommendations"</div>
                            <div class="error-detail">{error}</div>
                        </div>
                    }
                })
        }}
        {move || {
            recs.with(|r| r.as_ref().is_some_and(Vec::is_empty))
                .then(|| {
                    let text = if available.get() {
                        "The Recommendations task hasn’t produced any picks. Run it to generate the first one."
                    } else {
                        "Add the required recommendation service credentials to enable the first run."
                    };
                    view! {
                        <div class="rec-empty">
                            <div class="rec-empty-title">"No recommendations yet"</div>
                            <div class="muted">{text}</div>
                        </div>
                    }
                })
        }}
        {move || {
            let total = recs.with(|r| r.as_ref().map_or(0, Vec::len));
            (total > 0 && visible.with(|v| v.as_ref().is_some_and(Vec::is_empty)))
                .then(|| {
                    view! {
                        <div class="rec-empty">
                            "No picks match this filter. Choose another status to see more."
                        </div>
                    }
                })
        }}
        {move || {
            visible
                .with(|v| v.as_ref().is_some_and(|v| !v.is_empty()))
                .then(|| {
                    let highlight = highlight.clone();
                    view! {
                        <div class="rec-list" aria-label="Recommendations">
                            <For
                                each=move || show.visible.get()
                                key=|rec| format!("{}|{rec:?}", rec.recommendation_id)
                                children=move |rec| {
                                    let id = rec.recommendation_id.clone();
                                    let saving_key = id.clone();
                                    let highlighted = highlight.as_deref() == Some(id.as_str());
                                    view! {
                                        <RecommendationCard
                                            rec=rec
                                            saving=Signal::derive(move || {
                                                saving_id.with(|s| s.as_deref() == Some(saving_key.as_str()))
                                            })
                                            highlighted=highlighted
                                            on_feedback=Callback::new(move |feedback| {
                                                on_feedback(id.clone(), feedback)
                                            })
                                        />
                                    }
                                }
                            />
                        </div>
                        {move || {
                            show.has_more
                                .get()
                                .then(|| {
                                    view! {
                                        <ShowMoreButton
                                            remaining=show.remaining
                                            on_click=Callback::new(move |()| show.show_more())
                                        />
                                    }
                                })
                        }}
                    }
                })
        }}

        <RecommendationRuns task_name=TASK_NAME latest_run_id=latest_recommendation_run_id />
    }
}

/// `/media/:id`.
#[component]
pub fn MediaDetailPage(#[prop(into)] id: String) -> impl IntoView {
    let rec = RwSignal::new(None::<Recommendation>);
    let error = RwSignal::new(None::<String>);
    let saving = RwSignal::new(false);
    let note_text = RwSignal::new(String::new());
    let saving_note = RwSignal::new(false);
    let toast = use_toast();

    let load_id = id.clone();
    spawn_scoped(async move {
        match api::fetch_recommendation(&load_id).await {
            Ok(res) => {
                note_text.set(res.recommendation.feedback_note.clone().unwrap_or_default());
                rec.set(Some(res.recommendation));
            }
            Err(err) => error.set(Some(err.message().to_owned())),
        }
    });

    let feedback_id = id.clone();
    let on_feedback = Callback::new(move |feedback: RecommendationFeedback| {
        saving.set(true);
        let id = feedback_id.clone();
        spawn_detached(async move {
            match api::send_recommendation_feedback(&id, Some(feedback), None).await {
                Ok(result) => {
                    rec.set(Some(result.recommendation));
                    toast.show("Feedback saved", ToastKind::Info);
                }
                Err(err) => toast.show(err.message(), ToastKind::Error),
            }
            saving.set(false);
        });
    });
    let note_id = id.clone();
    let save_note = move |_| {
        saving_note.set(true);
        let id = note_id.clone();
        let note = note_text.get_untracked().trim().to_owned();
        spawn_detached(async move {
            match api::send_recommendation_feedback(&id, None, Some(&note)).await {
                Ok(result) => {
                    rec.set(Some(result.recommendation));
                    toast.show("Note saved", ToastKind::Info);
                }
                Err(err) => toast.show(err.message(), ToastKind::Error),
            }
            saving_note.set(false);
        });
    };

    move || {
        if let Some(error) = error.get() {
            return view! {
                <div class="error">
                    <div>"Failed to load this recommendation"</div>
                    <div class="error-detail">{error}</div>
                </div>
            }
            .into_any();
        }
        let Some(rec) = rec.get() else {
            return view! { <div class="loading">"Loading…"</div> }.into_any();
        };
        let rateable = can_rate(rec.status);
        let manager = if rec.media_type == MediaType::Movie {
            "Radarr"
        } else {
            "Sonarr"
        };
        let poster = rec
            .poster_path
            .as_ref()
            .map(|p| format!("https://image.tmdb.org/t/p/w342{p}"));
        let caveats = (!rec.caveats.is_empty()).then(|| {
            view! {
                <ul class="rec-caveats">
                    {rec.caveats.iter().map(|c| view! { <li>{c.clone()}</li> }).collect_view()}
                </ul>
            }
        });
        let note_form = rateable.then(|| {
            view! {
                <div class="feedback-note">
                    <label class="feedback-note-label" for="rec-feedback-note">
                        "Note"
                    </label>
                    <textarea
                        id="rec-feedback-note"
                        class="feedback-note-input"
                        placeholder="Optional note about this pick…"
                        prop:value=move || note_text.get()
                        on:input=move |event| note_text.set(event_target_value(&event))
                        disabled=move || saving_note.get()
                    ></textarea>
                    <button
                        type="button"
                        class="feedback-note-save-btn"
                        disabled=move || saving_note.get() || note_text.with(|t| t.trim().is_empty())
                        on:click=save_note.clone()
                    >
                        {move || if saving_note.get() { "Saving…" } else { "Save Note" }}
                    </button>
                </div>
            }
        });
        let details = media_details(&rec);
        let scores = rec.shortlist_scores.clone().map(|scores| {
            let risks = (!scores.risks.is_empty()).then(|| {
                view! {
                    <ul class="rec-caveats detail-risks">
                        {scores.risks.iter().map(|r| view! { <li>{r.clone()}</li> }).collect_view()}
                    </ul>
                }
            });
            view! {
                <section class="page-section">
                    <h2 class="section-title">"Shortlist Scores"</h2>
                    <div class="score-list">
                        <ScoreRow label="Taste Match" value=scores.taste_match />
                        <ScoreRow label="Novelty" value=scores.novelty />
                        <ScoreRow label="Effort Fit" value=scores.effort_fit />
                        <ScoreRow label="Composite" value=scores.composite />
                    </div>
                    {risks}
                </section>
            }
        });
        let feedback_label = rec.feedback.map(|f| {
            REC_FEEDBACK_ACTIONS
                .iter()
                .find(|(v, _)| *v == f)
                .map_or_else(|| f.as_str().to_owned(), |(_, l)| (*l).to_owned())
        });
        view! {
            <BackLink to="/media" label="All Media Picks" />
            <Toast toast=toast.toast />

            <div class="detail-head">
                <ImageWithFallback
                    src=poster
                    alt=format!("{} poster", rec.title)
                    class="detail-art"
                    placeholder_class="detail-art-placeholder"
                    placeholder=|| "🎬"
                />
                <div class="detail-head-body">
                    <h1 class="detail-title">{title_with_year(&rec)}</h1>
                    <div class="detail-badges">{status_badges(&rec)}</div>
                    {rec.why_for_user.clone().filter(|w| !w.is_empty()).map(|why| view! { <p class="rec-why">{why}</p> })}
                    {caveats}
                    <nav class="detail-service-links" aria-label="Title links">
                        <a
                            href=rec.links.plex.clone()
                            target="_blank"
                            rel="noreferrer"
                            class="detail-service-link detail-service-link-plex"
                            aria-label=format!("Open {} in Plex", rec.title)
                        >
                            <span class="detail-service-link-label">"Open in Plex"</span>
                            <span class="detail-service-link-hint">"Find in library"</span>
                        </a>
                        <a
                            href=rec.links.manager.clone()
                            target="_blank"
                            rel="noreferrer"
                            class=format!(
                                "detail-service-link detail-service-link-manager detail-service-link-{}",
                                manager.to_lowercase(),
                            )
                            aria-label=format!("Open {} in {manager}", rec.title)
                        >
                            <span class="detail-service-link-label">{format!("Open in {manager}")}</span>
                            <span class="detail-service-link-hint">"Manage library"</span>
                        </a>
                        <a
                            href=rec.links.tmdb.clone()
                            target="_blank"
                            rel="noreferrer"
                            class="detail-metadata-link detail-metadata-link-tmdb"
                        >
                            "View metadata on TMDB"
                        </a>
                    </nav>
                    {rateable.then(|| feedback_buttons(rec.feedback, saving.into(), on_feedback))}
                    {note_form}
                </div>
            </div>

            <div class="detail-sections">
                <section class="page-section">
                    <h2 class="section-title">"Details"</h2>
                    <dl class="detail-grid">{details}</dl>
                </section>
                {scores}
                <section class="page-section">
                    <h2 class="section-title">"Timeline"</h2>
                    <div class="timeline">
                        <TimelineRow label="Recommended" at=rec.recommended_at />
                        {rec.notified_at.map(|at| view! { <TimelineRow label="Notified" at=at /> })}
                        {rec.started_at.map(|at| view! { <TimelineRow label="Started Watching" at=at /> })}
                        {rec
                            .resolved_at
                            .map(|at| {
                                view! {
                                    <TimelineRow
                                        label=format!("Resolved ({})", rec_status_label(rec.status))
                                        at=at
                                    />
                                }
                            })}
                        {feedback_label
                            .zip(rec.feedback_at)
                            .map(|(label, at)| {
                                view! { <TimelineRow label=format!("Feedback: {label}") at=at /> }
                            })}
                    </div>
                </section>
            </div>
        }
        .into_any()
    }
}

fn media_details(rec: &Recommendation) -> impl IntoView + use<> {
    let field = |label: &str, value: String| {
        let label = label.to_owned();
        view! { <DetailField label=label>{value}</DetailField> }
    };
    let join = |items: &[String]| items.join(", ");
    let mut fields = Vec::new();
    if !rec.genres.is_empty() {
        fields.push(field("Genres", join(&rec.genres)));
    }
    if let Some(minutes) = rec.runtime_minutes {
        fields.push(field("Runtime", format_minutes(minutes)));
    }
    if let Some(seasons) = rec.season_count {
        let episodes = rec
            .episode_count
            .map(|e| format!(" ({} episodes)", number_string(e)))
            .unwrap_or_default();
        fields.push(field(
            "Seasons",
            format!("{}{episodes}", number_string(seasons)),
        ));
    }
    if let Some(status) = rec.series_status.clone().filter(|s| !s.is_empty()) {
        fields.push(field("Series Status", status));
    }
    if let Some(rated) = rec.certification.clone().filter(|s| !s.is_empty()) {
        fields.push(field("Rated", rated));
    }
    if let Some(language) = rec.original_language.as_ref().filter(|s| !s.is_empty()) {
        fields.push(field("Language", language.to_uppercase()));
    }
    if !rec.origin_countries.is_empty() {
        fields.push(field("Country", join(&rec.origin_countries)));
    }
    if !rec.creators.is_empty() {
        let label = if rec.media_type == MediaType::Movie {
            "Directed By"
        } else {
            "Created By"
        };
        fields.push(field(label, join(&rec.creators)));
    }
    if !rec.cast.is_empty() {
        fields.push(field("Cast", join(&rec.cast)));
    }
    if !rec.keywords.is_empty() {
        fields.push(field("Keywords", join(&rec.keywords)));
    }
    if let Some(source) = rec.source.clone().filter(|s| !s.is_empty()) {
        fields.push(field("Source", source));
    }
    if let Some(confidence) = rec.confidence {
        fields.push(field(
            "Confidence",
            format!("{}%", number_string(js_round(confidence * 100.0))),
        ));
    }
    fields
}
