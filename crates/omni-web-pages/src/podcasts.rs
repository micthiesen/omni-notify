//! Podcast picks (`pages/PodcastsPage.tsx`) and one pick's detail page
//! (`pages/PodcastDetailPage.tsx`).

use std::collections::HashMap;

use leptos::prelude::*;
use omni_api::common::encode_uri_component;
use omni_api::podcasts::{
    PodcastFeedback, PodcastQueueResult, PodcastRecommendation, PodcastRecommendationStatus,
    PodcastTasteProfile,
};
use omni_web_kit::api;
use omni_web_kit::components::{
    ImageWithFallback, ShowMoreButton, StatusFilterChips, Toast, ToastKind, use_show_more,
    use_toast,
};
use omni_web_kit::hooks::use_rec_highlight;
use omni_web_kit::live::use_live_data;
use omni_web_kit::router::Link;
use omni_web_kit::task::{spawn_detached, spawn_scoped};
use omni_web_kit::utils::format::{format_absolute, format_date_only, format_relative};
use omni_web_kit::utils::js::{js_round, number_string};
use omni_web_kit::utils::rec_labels::{
    PODCAST_FEEDBACK_ACTIONS, PODCAST_STATUS_ORDER, podcast_status_label, podcast_status_str,
};

use crate::common::{
    BackLink, DetailField, RunControls, ScoreRow, TimelineRow, active_if, format_minutes,
};
use crate::recommendation_runs::RecommendationRuns;
use crate::taste_brain::{TasteBrain, TasteBrainProfile};

const TASK_NAME: &str = "PodcastRecs";
const TASTE_TASK_NAME: &str = "PodcastTasteReflection";

fn feedback_str(feedback: PodcastFeedback) -> &'static str {
    match feedback {
        PodcastFeedback::GoodPick => "good_pick",
        PodcastFeedback::NotForMe => "not_for_me",
    }
}

fn can_rate(status: PodcastRecommendationStatus) -> bool {
    status != PodcastRecommendationStatus::Pending && status != PodcastRecommendationStatus::Failed
}

fn in_castro_queue(rec: &PodcastRecommendation) -> bool {
    matches!(
        rec.queue_result,
        Some(PodcastQueueResult::Queued | PodcastQueueResult::AlreadyQueued)
    )
}

fn status_chip(status: PodcastRecommendationStatus) -> impl IntoView {
    view! {
        <span class=format!("status-chip status-chip-{}", podcast_status_str(status))>
            {podcast_status_label(status)}
        </span>
    }
}

fn feedback_buttons(
    current: Option<PodcastFeedback>,
    saving: Signal<bool>,
    on_feedback: Callback<PodcastFeedback>,
) -> impl IntoView {
    view! {
        <div class="rec-feedback" aria-label="Recommendation feedback">
            {PODCAST_FEEDBACK_ACTIONS
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

fn mic_icon() -> impl IntoView {
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
            <rect x="9" y="2" width="6" height="12" rx="3"></rect>
            <path d="M5 10a7 7 0 0 0 14 0"></path>
            <path d="M12 17v4M9 21h6"></path>
        </svg>
    }
}

#[component]
fn PodcastCard(
    rec: PodcastRecommendation,
    #[prop(into)] saving: Signal<bool>,
    highlighted: bool,
    on_feedback: Callback<PodcastFeedback>,
) -> impl IntoView {
    let caveats = (!rec.caveats.is_empty()).then(|| {
        view! {
            <details class="content-disclosure rec-caveat-disclosure">
                <summary>"Before You Listen"</summary>
                <ul class="rec-caveats">
                    {rec.caveats.iter().map(|c| view! { <li>{c.clone()}</li> }).collect_view()}
                </ul>
            </details>
        }
    });
    let episode_url = rec.episode_url.clone().filter(|u| !u.is_empty());
    let source_url = rec.source_url.clone().filter(|u| !u.is_empty());
    let links = (episode_url.is_some() || source_url.is_some()).then(|| {
        view! {
            <div class="rec-links">
                {episode_url
                    .map(|url| {
                        view! {
                            <a class="content-primary-link" href=url target="_blank" rel="noreferrer">
                                "Open Episode ↗"
                            </a>
                        }
                    })}
                {source_url
                    .map(|url| {
                        view! {
                            <a href=url target="_blank" rel="noreferrer">
                                "Discussion"
                            </a>
                        }
                    })}
            </div>
        }
    });
    let recommended = rec.recommended_at as f64;
    let episode_title = rec.episode_title.clone();
    view! {
        <div
            id=format!("recommendation-{}", rec.recommendation_id)
            class=format!("rec-card {}", if highlighted { "rec-card-highlighted" } else { "" })
        >
            <ImageWithFallback
                src=rec.artwork_url.clone()
                alt=format!("{} artwork", rec.show_title)
                class="podrec-artwork"
                placeholder_class="podrec-artwork-placeholder"
                lazy=true
                placeholder=mic_icon
            />
            <div class="rec-body">
                <div class="rec-title-row">
                    <Link
                        to=format!("/podcasts/{}", encode_uri_component(&rec.recommendation_id))
                        class="rec-title rec-title-link"
                        title="View details"
                    >
                        {episode_title}
                    </Link>
                </div>
                <div class="rec-badges">
                    {status_chip(rec.status)}
                    {in_castro_queue(&rec)
                        .then(|| {
                            view! {
                                <span class="podrec-queued" title="This episode is waiting in your Castro queue">
                                    "In Castro Queue"
                                </span>
                            }
                        })}
                </div>
                <div class="podrec-show">{rec.show_title.clone()}</div>
                {(!rec.matched_voices.is_empty())
                    .then(|| {
                        view! {
                            <div class="podrec-featuring">
                                {format!("Featuring {}", rec.matched_voices.join(", "))}
                            </div>
                        }
                    })}
                {rec.why_for_user.clone().filter(|w| !w.is_empty()).map(|why| view! { <p class="rec-why">{why}</p> })}
                {caveats}
                <div class="rec-meta meta-row">
                    <span>{format!("Released {}", format_date_only(rec.published_at as f64))}</span>
                    {rec
                        .duration_minutes
                        .map(|m| view! { <span class="muted">{format_minutes(m as f64)}</span> })}
                    <span class="muted" title=format_absolute(recommended)>
                        {format!("Recommended {}", format_relative(recommended))}
                    </span>
                </div>
                {links}
                {can_rate(rec.status).then(|| feedback_buttons(rec.feedback, saving, on_feedback))}
            </div>
        </div>
    }
}

fn podcast_stats(profile: &PodcastTasteProfile) -> Vec<(String, String)> {
    let stats = &profile.stats;
    vec![
        (
            "Episodes Finished".into(),
            stats.listened_episodes.to_string(),
        ),
        (
            "Episodes Started".into(),
            stats.started_episodes.to_string(),
        ),
        ("Starred".into(), stats.starred_episodes.to_string()),
        ("Shows Heard".into(), stats.distinct_shows.to_string()),
        (
            "Recommendations Listened".into(),
            format!(
                "{}/{}",
                stats.recommendations.listened, stats.recommendations.total
            ),
        ),
        ("Good Picks".into(), stats.feedback.good_pick.to_string()),
    ]
}

#[component]
pub fn PodcastsPage() -> impl IntoView {
    let recs = RwSignal::new(None::<Vec<PodcastRecommendation>>);
    let recs_error = RwSignal::new(None::<String>);
    let status_filter = RwSignal::new(String::new());
    let saving_id = RwSignal::new(None::<String>);
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

    // Load once, then reload whenever a reflection run lands.
    Effect::new(move |_| {
        latest_taste_run_id.track();
        spawn_scoped(async move {
            match api::fetch_podcast_taste_profile().await {
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

    Effect::new(move |_| {
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

    let on_feedback = move |recommendation_id: String, feedback: PodcastFeedback| {
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

    let highlighted_id = use_rec_highlight(Signal::derive(move || recs.with(Option::is_some)));

    let status_counts = Memo::new(move |_| {
        let mut counts = HashMap::<String, usize>::new();
        recs.with(|list| {
            for rec in list.iter().flatten() {
                *counts
                    .entry(podcast_status_str(rec.status).to_owned())
                    .or_default() += 1;
            }
        });
        counts
    });
    let visible = Memo::new(move |_| {
        let filter = status_filter.get();
        recs.with(|list| {
            list.as_ref().map(|list| {
                list.iter()
                    .filter(|r| filter.is_empty() || podcast_status_str(r.status) == filter)
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
        taste_profile.with(|p| p.as_ref().map(podcast_stats).unwrap_or_default())
    });
    let order: Vec<(String, String)> = PODCAST_STATUS_ORDER
        .iter()
        .map(|s| {
            (
                podcast_status_str(*s).to_owned(),
                podcast_status_label(*s).to_owned(),
            )
        })
        .collect();
    let highlight = highlighted_id.clone();

    view! {
        <div class="page-header">
            <div class="page-header-stack">
                <h1>"Podcast Picks"</h1>
                <p class="page-subtitle">"Fresh voices and worthwhile conversations, picked for you."</p>
            </div>
            <RunControls
                task_name=TASK_NAME
                select_label="Maximum podcast recommendations"
                max_options=5
                disabled_title="Task disabled: missing podcast recommendation configuration"
                running=running
                available=available
                live=live
                toast=toast
            />
        </div>
        <Toast toast=toast.toast />

        <TasteBrain
            profile=taste
            loading=taste_loading
            error=taste_error
            subtitle="What your listening and feedback say about your taste."
            empty_text="No profile yet. The reflection task will build one from Castro listen history and recommendation feedback."
            stats=taste_stats
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
                            <div>"Failed to load podcast recommendations"</div>
                            <div class="error-detail">{error}</div>
                        </div>
                    }
                })
        }}
        {move || {
            recs.with(|r| r.as_ref().is_some_and(Vec::is_empty))
                .then(|| {
                    view! {
                        <div class="rec-empty">
                            <div class="rec-empty-title">"No podcast recommendations yet"</div>
                            <div class="muted">
                                "The podcast recommendation task hasn’t produced any picks yet."
                            </div>
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
                                        <PodcastCard
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

/// `/podcasts/:id`.
#[component]
pub fn PodcastDetailPage(#[prop(into)] id: String) -> impl IntoView {
    let rec = RwSignal::new(None::<PodcastRecommendation>);
    let error = RwSignal::new(None::<String>);
    let saving = RwSignal::new(false);
    let note_text = RwSignal::new(String::new());
    let saving_note = RwSignal::new(false);
    let toast = use_toast();

    let load_id = id.clone();
    spawn_scoped(async move {
        match api::fetch_podcast_recommendation(&load_id).await {
            Ok(res) => {
                note_text.set(res.recommendation.feedback_note.clone().unwrap_or_default());
                rec.set(Some(res.recommendation));
            }
            Err(err) => error.set(Some(err.message().to_owned())),
        }
    });

    let feedback_id = id.clone();
    let on_feedback = Callback::new(move |feedback: PodcastFeedback| {
        saving.set(true);
        let id = feedback_id.clone();
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
    let note_id = id.clone();
    let save_note = move |_| {
        saving_note.set(true);
        let id = note_id.clone();
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
        let caveats = (!rec.caveats.is_empty()).then(|| {
            view! {
                <ul class="rec-caveats">
                    {rec.caveats.iter().map(|c| view! { <li>{c.clone()}</li> }).collect_view()}
                </ul>
            }
        });
        let episode_url = rec.episode_url.clone().filter(|u| !u.is_empty());
        let source_url = rec.source_url.clone().filter(|u| !u.is_empty());
        let links = (episode_url.is_some() || source_url.is_some()).then(|| {
            let title = rec.episode_title.clone();
            view! {
                <nav class="detail-service-links" aria-label="Episode links">
                    {episode_url
                        .map(|url| {
                            view! {
                                <a
                                    href=url
                                    target="_blank"
                                    rel="noreferrer"
                                    class="detail-service-link"
                                    aria-label=format!("Open {title}")
                                >
                                    <span class="detail-service-link-label">"Open Episode"</span>
                                    <span class="detail-service-link-hint">"Listen on the episode site"</span>
                                </a>
                            }
                        })}
                    {source_url
                        .map(|url| {
                            view! {
                                <a href=url target="_blank" rel="noreferrer" class="detail-metadata-link">
                                    "Discussion"
                                </a>
                            }
                        })}
                </nav>
            }
        });
        let note_form = rateable.then(|| {
            view! {
                <div class="feedback-note">
                    <label class="feedback-note-label" for="podrec-feedback-note">
                        "Note"
                    </label>
                    <textarea
                        id="podrec-feedback-note"
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
        let mut details = vec![
            view! { <DetailField label="Released">{format_date_only(rec.published_at as f64)}</DetailField> }
                .into_any(),
        ];
        if let Some(minutes) = rec.duration_minutes {
            details.push(
                view! { <DetailField label="Duration">{format_minutes(minutes as f64)}</DetailField> }
                    .into_any(),
            );
        }
        if let Some(via) = rec.discovered_via.clone().filter(|v| !v.is_empty()) {
            details
                .push(view! { <DetailField label="Discovered Via">{via}</DetailField> }.into_any());
        }
        if let Some(confidence) = rec.confidence {
            details.push(
                view! {
                    <DetailField label="Confidence">
                        {format!("{}%", number_string(js_round(confidence * 100.0)))}
                    </DetailField>
                }
                .into_any(),
            );
        }
        if let Some(itunes) = rec.itunes_id {
            details.push(
                view! { <DetailField label="iTunes ID">{itunes.to_string()}</DetailField> }
                    .into_any(),
            );
        }
        let feed_url = rec.feed_url.clone();
        let feed_text = rec.feed_url.clone();
        details.push(
            view! {
                <DetailField label="Feed">
                    <a href=feed_url target="_blank" rel="noreferrer" class="detail-feed-url">
                        {feed_text}
                    </a>
                </DetailField>
            }
            .into_any(),
        );
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
                        <ScoreRow label="Composite" value=scores.composite />
                    </div>
                    {risks}
                </section>
            }
        });
        let feedback_label = rec.feedback.map(|f| {
            PODCAST_FEEDBACK_ACTIONS
                .iter()
                .find(|(v, _)| *v == f)
                .map_or_else(|| feedback_str(f).to_owned(), |(_, l)| (*l).to_owned())
        });
        view! {
            <BackLink to="/podcasts" label="All Podcast Picks" />
            <Toast toast=toast.toast />

            <div class="detail-head">
                <ImageWithFallback
                    src=rec.artwork_url.clone()
                    alt=format!("{} artwork", rec.show_title)
                    class="detail-art detail-art-square"
                    placeholder_class="detail-art-placeholder"
                    placeholder=|| "🎧"
                />
                <div class="detail-head-body">
                    <h1 class="detail-title">{rec.episode_title.clone()}</h1>
                    <div class="podrec-show">{rec.show_title.clone()}</div>
                    <div class="detail-badges">
                        {status_chip(rec.status)}
                        {in_castro_queue(&rec)
                            .then(|| {
                                view! {
                                    <span class="podrec-queued" title="This episode is waiting in your Castro queue">
                                        "🎧 In Castro Queue"
                                    </span>
                                }
                            })}
                    </div>
                    {(!rec.matched_voices.is_empty())
                        .then(|| {
                            view! {
                                <div class="podrec-featuring">
                                    {format!("🎙️ Featuring {}", rec.matched_voices.join(", "))}
                                </div>
                            }
                        })}
                    {rec.why_for_user.clone().filter(|w| !w.is_empty()).map(|why| view! { <p class="rec-why">{why}</p> })}
                    {caveats}
                    {links}
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
                        <TimelineRow label="Recommended" at=rec.recommended_at as f64 />
                        {rec.notified_at.map(|at| view! { <TimelineRow label="Notified" at=at as f64 /> })}
                        {feedback_label
                            .zip(rec.feedback_at)
                            .map(|(label, at)| {
                                view! { <TimelineRow label=format!("Feedback: {label}") at=at as f64 /> }
                            })}
                    </div>
                </section>
            </div>
        }
        .into_any()
    }
}
