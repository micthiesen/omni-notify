//! One-tap feedback cards linked from Pushover notifications.

use leptos::prelude::*;
use omni_api::common::encode_uri_component;
use omni_api::media::{MediaType, RecommendationFeedback};
use omni_api::podcasts::PodcastFeedback;
use omni_web_kit::api::{self, ApiClientError};
use omni_web_kit::components::ImageWithFallback;
use omni_web_kit::router::Link;
use omni_web_kit::task::{spawn_detached, spawn_scoped};
use omni_web_kit::utils::js::number_string;

use crate::common::active_if;

/// Which feedback form `/feedback/:kind/:id` shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeedbackKind {
    Recommendations,
    Podcasts,
}

impl FeedbackKind {
    /// The route segment (`recommendations` or `podcasts`).
    pub fn from_segment(segment: &str) -> Option<Self> {
        match segment {
            "recommendations" => Some(Self::Recommendations),
            "podcasts" => Some(Self::Podcasts),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Recommendations => "recommendations",
            Self::Podcasts => "podcasts",
        }
    }
}

/// A rating option: the serialized value, emoji and label.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FeedbackOption {
    value: &'static str,
    emoji: &'static str,
    label: &'static str,
}

const MEDIA_OPTIONS: [FeedbackOption; 3] = [
    FeedbackOption {
        value: "good_pick",
        emoji: "👍",
        label: "Good Pick",
    },
    FeedbackOption {
        value: "not_for_me",
        emoji: "👎",
        label: "Not for Me",
    },
    FeedbackOption {
        value: "already_watched",
        emoji: "✅",
        label: "Already Watched",
    },
];

const PODCAST_OPTIONS: [FeedbackOption; 2] = [
    FeedbackOption {
        value: "good_pick",
        emoji: "👍",
        label: "Good Pick",
    },
    FeedbackOption {
        value: "not_for_me",
        emoji: "👎",
        label: "Not for Me",
    },
];

/// What the shell shows for the loaded recommendation.
#[derive(Clone, Debug, PartialEq)]
struct ShellData {
    art_src: Option<String>,
    art_alt: String,
    title: String,
    subtitle: Option<String>,
    why: Option<String>,
    current: Option<&'static str>,
    note: Option<String>,
    details_to: String,
}

/// Saves a rating (`Some(value)`) or a note (`None`, with the note text).
type SaveFn = Callback<(Option<&'static str>, Option<String>), ()>;

fn media_value(value: &str) -> Option<RecommendationFeedback> {
    match value {
        "good_pick" => Some(RecommendationFeedback::GoodPick),
        "not_for_me" => Some(RecommendationFeedback::NotForMe),
        "already_watched" => Some(RecommendationFeedback::AlreadyWatched),
        _ => None,
    }
}

fn podcast_value(value: &str) -> Option<PodcastFeedback> {
    match value {
        "good_pick" => Some(PodcastFeedback::GoodPick),
        "not_for_me" => Some(PodcastFeedback::NotForMe),
        _ => None,
    }
}

async fn load(kind: FeedbackKind, id: &str) -> Result<ShellData, ApiClientError> {
    match kind {
        FeedbackKind::Recommendations => {
            let rec = api::fetch_recommendation(id).await?.recommendation;
            Ok(media_shell(&rec, id))
        }
        FeedbackKind::Podcasts => {
            let rec = api::fetch_podcast_recommendation(id).await?.recommendation;
            Ok(podcast_shell(&rec, id))
        }
    }
}

fn media_shell(rec: &omni_api::media::Recommendation, id: &str) -> ShellData {
    ShellData {
        art_src: rec
            .poster_path
            .as_ref()
            .map(|p| format!("https://image.tmdb.org/t/p/w185{p}")),
        art_alt: format!("{} poster", rec.title),
        title: match rec.year {
            Some(year) if year != 0.0 => format!("{} ({})", rec.title, number_string(year)),
            _ => rec.title.clone(),
        },
        subtitle: Some(if rec.media_type == MediaType::Movie {
            "Movie".to_owned()
        } else {
            "Series".to_owned()
        }),
        why: rec.why_for_user.clone(),
        current: rec.feedback.map(|f| f.as_str()),
        note: rec.feedback_note.clone(),
        details_to: format!("/media/{}", encode_uri_component(id)),
    }
}

fn podcast_shell(rec: &omni_api::podcasts::PodcastRecommendation, id: &str) -> ShellData {
    ShellData {
        art_src: rec.artwork_url.clone(),
        art_alt: format!("{} artwork", rec.show_title),
        title: rec.episode_title.clone(),
        subtitle: Some(rec.show_title.clone()),
        why: rec.why_for_user.clone(),
        current: rec.feedback.map(|f| match f {
            PodcastFeedback::GoodPick => "good_pick",
            PodcastFeedback::NotForMe => "not_for_me",
        }),
        note: rec.feedback_note.clone(),
        details_to: format!("/podcasts/{}", encode_uri_component(id)),
    }
}

async fn save(
    kind: FeedbackKind,
    id: &str,
    value: Option<&'static str>,
    note: Option<&str>,
) -> Result<ShellData, ApiClientError> {
    match kind {
        FeedbackKind::Recommendations => {
            let rec = api::send_recommendation_feedback(id, value.and_then(media_value), note)
                .await?
                .recommendation;
            Ok(media_shell(&rec, id))
        }
        FeedbackKind::Podcasts => {
            let rec =
                api::send_podcast_recommendation_feedback(id, value.and_then(podcast_value), note)
                    .await?
                    .recommendation;
            Ok(podcast_shell(&rec, id))
        }
    }
}

#[component]
fn FeedbackShell(
    #[prop(into)] data: Signal<ShellData>,
    options: &'static [FeedbackOption],
    on_save: SaveFn,
    saving: RwSignal<bool>,
    saved: RwSignal<bool>,
    save_error: RwSignal<Option<String>>,
) -> impl IntoView {
    let note_text = RwSignal::new(data.with_untracked(|d| d.note.clone().unwrap_or_default()));
    let select = move |value: &'static str| {
        if saving.get_untracked() {
            return;
        }
        on_save.run((Some(value), None));
    };
    let save_note = move |_| {
        if saving.get_untracked() {
            return;
        }
        on_save.run((None, Some(note_text.get_untracked())));
    };
    let header = move || {
        let d = data.get();
        view! {
            <ImageWithFallback
                src=d.art_src.clone()
                alt=d.art_alt.clone()
                class="feedback-art"
                placeholder_class="feedback-art-placeholder"
                placeholder=|| "🎯"
            />
            <h1 class="feedback-title">{d.title.clone()}</h1>
            {d.subtitle.clone().filter(|s| !s.is_empty()).map(|s| view! { <div class="feedback-sub">{s}</div> })}
            {d.why.clone().filter(|s| !s.is_empty()).map(|why| view! { <p class="feedback-why">{why}</p> })}
        }
    };
    let buttons = options
        .iter()
        .map(|option| {
            let value = option.value;
            let pressed = move || data.with(|d| d.current == Some(value));
            view! {
                <button
                    type="button"
                    class=move || format!("feedback-option {}", active_if(pressed()))
                    aria-pressed=move || pressed().to_string()
                    disabled=move || saving.get()
                    on:click=move |_| select(value)
                >
                    <span aria-hidden="true">{option.emoji}</span>
                    {option.label}
                </button>
            }
        })
        .collect_view();
    view! {
        <div class="feedback-card">
            {header}
            <div class="feedback-options">{buttons}</div>
            <div class="feedback-note">
                <textarea
                    class="feedback-note-input"
                    placeholder="Optional note about this pick…"
                    prop:value=move || note_text.get()
                    on:input=move |event| note_text.set(event_target_value(&event))
                    disabled=move || saving.get()
                ></textarea>
                <button
                    type="button"
                    class="feedback-note-save-btn"
                    disabled=move || saving.get() || note_text.with(|t| t.trim().is_empty())
                    on:click=save_note
                >
                    "Save Note"
                </button>
            </div>
            {move || {
                (saved.get() && save_error.with(Option::is_none))
                    .then(|| view! { <div class="feedback-saved">"Thanks — feedback saved."</div> })
            }}
            {move || save_error.get().map(|e| view! { <div class="error-inline">{e}</div> })}
            <Link to=data.with_untracked(|d| d.details_to.clone()) class="feedback-details-link">
                "View Full Recommendation →"
            </Link>
        </div>
    }
}

/// `/feedback/(recommendations|podcasts)/:id`.
#[component]
pub fn FeedbackPage(kind: FeedbackKind, #[prop(into)] id: String) -> impl IntoView {
    let data = RwSignal::new(None::<ShellData>);
    let error = RwSignal::new(None::<String>);
    let saving = RwSignal::new(false);
    let saved = RwSignal::new(false);
    let save_error = RwSignal::new(None::<String>);

    let load_id = id.clone();
    spawn_scoped(async move {
        match load(kind, &load_id).await {
            Ok(shell) => data.set(Some(shell)),
            Err(err) => error.set(Some(err.message().to_owned())),
        }
    });

    let on_save: SaveFn = Callback::new(
        move |(value, note): (Option<&'static str>, Option<String>)| {
            saving.set(true);
            save_error.set(None);
            let id = id.clone();
            spawn_detached(async move {
                match save(kind, &id, value, note.as_deref()).await {
                    Ok(shell) => {
                        data.set(Some(shell));
                        saved.set(true);
                    }
                    Err(err) => save_error.set(Some(err.message().to_owned())),
                }
                saving.set(false);
            });
        },
    );

    let loaded = Memo::new(move |_| data.with(Option::is_some));
    view! {
        <div class="feedback-page">
            {move || {
                if let Some(error) = error.get() {
                    return view! {
                        <div class="error">
                            <div>"Couldn't load this recommendation"</div>
                            <div class="error-detail">{error}</div>
                        </div>
                    }
                    .into_any();
                }
                if !loaded.get() {
                    return view! { <div class="loading">"Loading…"</div> }.into_any();
                }
                let options: &'static [FeedbackOption] = match kind {
                    FeedbackKind::Recommendations => &MEDIA_OPTIONS,
                    FeedbackKind::Podcasts => &PODCAST_OPTIONS,
                };
                let shell = Signal::derive(move || {
                    data.get().unwrap_or_else(|| ShellData {
                        art_src: None,
                        art_alt: String::new(),
                        title: String::new(),
                        subtitle: None,
                        why: None,
                        current: None,
                        note: None,
                        details_to: String::new(),
                    })
                });
                view! {
                    <FeedbackShell
                        data=shell
                        options=options
                        on_save=on_save
                        saving=saving
                        saved=saved
                        save_error=save_error
                    />
                }
                .into_any()
            }}
        </div>
    }
}
