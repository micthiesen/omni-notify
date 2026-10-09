//! One-tap feedback linked from Pushover notifications: a focused single
//! column (no rail, no tab bar) that works one-handed at 390 px.

use leptos::prelude::*;
use omni_api::common::encode_uri_component;
use omni_api::media::{MediaType, RecommendationFeedback};
use omni_api::podcasts::PodcastFeedback;
use omni_web_kit::api::{self, ApiClientError};
use omni_web_kit::components::{
    Button, ButtonLink, ButtonSize, ButtonVariant, ErrorState, InlineNote, Poster, Skeleton,
    SkeletonKind, Tone,
};
use omni_web_kit::task::{spawn_detached, spawn_scoped};
use omni_web_kit::utils::js::number_string;

use crate::rec_ui::{Choice, ChoiceGlyph, ChoiceStyle, Choices};

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

/// The serialized values (`good_pick`, …) with their labels and glyphs.
fn media_options() -> Vec<Choice<&'static str>> {
    vec![
        Choice {
            value: "good_pick",
            label: "Good pick",
            glyph: ChoiceGlyph::Up,
        },
        Choice {
            value: "not_for_me",
            label: "Not for me",
            glyph: ChoiceGlyph::Down,
        },
        Choice {
            value: "already_watched",
            label: "Already watched",
            glyph: ChoiceGlyph::Seen,
        },
    ]
}

fn podcast_options() -> Vec<Choice<&'static str>> {
    media_options().into_iter().take(2).collect()
}

fn option_label(value: &str) -> &'static str {
    match value {
        "good_pick" => "Good pick",
        "not_for_me" => "Not for me",
        "already_watched" => "Already watched",
        _ => "Saved",
    }
}

/// What the shell shows for the loaded recommendation.
#[derive(Clone, Debug, PartialEq)]
struct ShellData {
    art_src: Option<String>,
    /// Podcast artwork is square; movie posters are 2:3.
    square: bool,
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
            .map(|p| format!("https://image.tmdb.org/t/p/w342{p}")),
        square: false,
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
        square: true,
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

/// What the last successful save changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Saved {
    Rating,
    Note,
}

fn check_mark() -> impl IntoView {
    view! {
        <svg class="fb-check" viewBox="0 0 48 48" aria-hidden="true">
            <circle cx="24" cy="24" r="21"></circle>
            <path d="m15 24.5 6.5 6.5L33.5 18"></path>
        </svg>
    }
}

#[component]
fn FeedbackShell(
    data: ShellData,
    #[prop(into)] current: Signal<Option<&'static str>>,
    options: Vec<Choice<&'static str>>,
    on_save: SaveFn,
    saving: RwSignal<bool>,
    saved: RwSignal<Option<Saved>>,
    save_error: RwSignal<Option<String>>,
) -> impl IntoView {
    let note_text = RwSignal::new(data.note.clone().unwrap_or_default());
    // The choices stay hidden behind the confirmation until "Change".
    let changing = RwSignal::new(false);
    let pick = Callback::new(move |value: &'static str| {
        if saving.get_untracked() {
            return;
        }
        changing.set(false);
        on_save.run((Some(value), None));
    });
    let save_note = Callback::new(move |_| {
        if saving.get_untracked() {
            return;
        }
        on_save.run((None, Some(note_text.get_untracked())));
    });
    let confirmed = Memo::new(move |_| {
        saved.get() == Some(Saved::Rating) && current.get().is_some() && !changing.get()
    });
    let blank = Signal::derive(move || note_text.with(|t| t.trim().is_empty()));
    let ShellData {
        art_src,
        square,
        title,
        subtitle,
        why,
        details_to,
        ..
    } = data;
    view! {
        <article class="fb">
            <header class="fb-head">
                <div class=if square { "fb-art square" } else { "fb-art" }>
                    <Poster src=art_src title=title.clone() square eager=true/>
                </div>
                <div class="fb-title">
                    {subtitle.filter(|s| !s.is_empty()).map(|s| view! { <span class="label">{s}</span> })}
                    <h1>{title}</h1>
                </div>
            </header>
            {why.filter(|s| !s.is_empty()).map(|why| view! { <p class="fb-why">{why}</p> })}
            {move || {
                if confirmed.get() {
                    let label = option_label(current.get().unwrap_or_default());
                    view! {
                        <div class="fb-saved" role="status">
                            {check_mark()}
                            <div class="fb-saved-text">
                                <span class="label">"Saved"</span>
                                <strong>{label}</strong>
                            </div>
                            <Button
                                size=ButtonSize::Sm
                                variant=ButtonVariant::Ghost
                                on_click=Callback::new(move |_| changing.set(true))
                            >
                                "Change"
                            </Button>
                        </div>
                    }
                    .into_any()
                } else {
                    view! {
                        <Choices
                            choices=options.clone()
                            current
                            saving
                            on_pick=pick
                            style=ChoiceStyle::Large
                            aria_label="Rate this pick"
                        />
                    }
                    .into_any()
                }
            }}
            <div class="field">
                <label class="field-label" for="fb-note">"Note (optional)"</label>
                <textarea
                    id="fb-note"
                    class="textarea"
                    rows="3"
                    placeholder="Anything to remember about this pick…"
                    prop:value=move || note_text.get()
                    on:input=move |event| note_text.set(event_target_value(&event))
                    disabled=move || saving.get()
                ></textarea>
            </div>
            <Button
                block=true
                busy=Signal::derive(move || saving.get())
                disabled=blank
                disabled_reason="Write a note first"
                on_click=save_note
            >
                "Save note"
            </Button>
            {move || {
                (saved.get() == Some(Saved::Note) && save_error.with(Option::is_none))
                    .then(|| view! { <InlineNote tone=Tone::Ok role="status">"Note saved."</InlineNote> })
            }}
            {move || save_error.get().map(|e| view! {
                <ErrorState title="That didn't save. Try again." raw=e/>
            })}
            <ButtonLink to=details_to variant=ButtonVariant::Ghost block=true>
                "See details"
            </ButtonLink>
        </article>
    }
}

/// `/feedback/(recommendations|podcasts)/:id`.
#[component]
pub fn FeedbackPage(kind: FeedbackKind, #[prop(into)] id: String) -> impl IntoView {
    let data = RwSignal::new(None::<ShellData>);
    let error = RwSignal::new(None::<String>);
    let reload = RwSignal::new(0u32);
    let saving = RwSignal::new(false);
    let saved = RwSignal::new(None::<Saved>);
    let save_error = RwSignal::new(None::<String>);

    let load_id = id.clone();
    Effect::new(move |_| {
        reload.track();
        let id = load_id.clone();
        spawn_scoped(async move {
            match load(kind, &id).await {
                Ok(shell) => {
                    data.set(Some(shell));
                    error.set(None);
                }
                Err(err) => error.set(Some(err.message().to_owned())),
            }
        });
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
                        saved.set(Some(if value.is_some() {
                            Saved::Rating
                        } else {
                            Saved::Note
                        }));
                    }
                    Err(err) => save_error.set(Some(err.message().to_owned())),
                }
                saving.set(false);
            });
        },
    );

    let loaded = Memo::new(move |_| data.with(Option::is_some));
    let current = Signal::derive(move || data.with(|d| d.as_ref().and_then(|d| d.current)));
    view! {
        <div class="fb-page">
            {move || {
                if let Some(err) = error.get().filter(|_| !loaded.get()) {
                    return view! {
                        <ErrorState
                            title="This recommendation could not load"
                            raw=err
                            retry=Callback::new(move |()| reload.update(|n| *n += 1))
                            page=true
                        />
                    }
                    .into_any();
                }
                if !loaded.get() {
                    return view! {
                        <div class="fb" role="status" aria-label="Loading">
                            <div class="fb-head">
                                <div class="fb-art"><Skeleton kind=SkeletonKind::Poster/></div>
                                <div class="fb-title stack"><Skeleton width="40%"/><Skeleton kind=SkeletonKind::Title/></div>
                            </div>
                            <Skeleton width="90%"/>
                            <Skeleton width="70%"/>
                        </div>
                    }
                    .into_any();
                }
                let options = match kind {
                    FeedbackKind::Recommendations => media_options(),
                    FeedbackKind::Podcasts => podcast_options(),
                };
                let shell = data.get_untracked().unwrap_or_else(|| unreachable!("loaded"));
                view! {
                    <FeedbackShell
                        data=shell
                        current
                        options
                        on_save
                        saving
                        saved
                        save_error
                    />
                }
                .into_any()
            }}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_round_trip() {
        for kind in [FeedbackKind::Recommendations, FeedbackKind::Podcasts] {
            assert_eq!(FeedbackKind::from_segment(kind.as_str()), Some(kind));
        }
        assert_eq!(FeedbackKind::from_segment("other"), None);
    }

    #[test]
    fn option_values_parse_for_their_kind() {
        for option in media_options() {
            assert!(media_value(option.value).is_some(), "{}", option.value);
            assert_eq!(option_label(option.value), option.label);
        }
        for option in podcast_options() {
            assert!(podcast_value(option.value).is_some(), "{}", option.value);
        }
        assert!(podcast_value("already_watched").is_none());
    }
}
