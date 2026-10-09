//! Pieces shared by the recommendation pages (Movies & TV, the one-tap
//! feedback page, Podcasts): rating choices and their glyphs, recommendation
//! status shapes and the phone media switch.

use leptos::prelude::*;
use omni_api::media::{RecommendationFeedback, RecommendationStatus};
use omni_web_kit::components::StatusKind;
use omni_web_kit::router::{Link, use_path};

/// The glyph drawn beside a rating choice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChoiceGlyph {
    Up,
    Down,
    Seen,
}

/// A stroked 24 px glyph in the kit's icon style.
pub fn choice_glyph(glyph: ChoiceGlyph, small: bool) -> impl IntoView {
    let shapes = match glyph {
        ChoiceGlyph::Up => view! {
            <path d="M7 10.5V20H4v-9.5zM7 10.5 11 3.5a2 2 0 0 1 3.6 1.6L13.6 10h5a2 2 0 0 1 2 2.4l-1.3 6A2 2 0 0 1 17.3 20H7"></path>
        }
        .into_any(),
        ChoiceGlyph::Down => view! {
            <path d="M7 13.5V4H4v9.5zM7 13.5l4 7a2 2 0 0 0 3.6-1.6L13.6 14h5a2 2 0 0 0 2-2.4l-1.3-6A2 2 0 0 0 17.3 4H7"></path>
        }
        .into_any(),
        ChoiceGlyph::Seen => view! {
            <path d="M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12z"></path>
            <circle cx="12" cy="12" r="2.75"></circle>
        }
        .into_any(),
    };
    view! {
        <svg class=if small { "icon sm" } else { "icon" } viewBox="0 0 24 24" aria-hidden="true">
            {shapes}
        </svg>
    }
}

/// One rating option.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Choice<T> {
    pub value: T,
    pub label: &'static str,
    pub glyph: ChoiceGlyph,
}

/// The media rating options, in display order.
pub fn media_choices() -> Vec<Choice<RecommendationFeedback>> {
    vec![
        Choice {
            value: RecommendationFeedback::GoodPick,
            label: "Good pick",
            glyph: ChoiceGlyph::Up,
        },
        Choice {
            value: RecommendationFeedback::NotForMe,
            label: "Not for me",
            glyph: ChoiceGlyph::Down,
        },
        Choice {
            value: RecommendationFeedback::AlreadyWatched,
            label: "Already watched",
            glyph: ChoiceGlyph::Seen,
        },
    ]
}

/// How [`Choices`] lays out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChoiceStyle {
    /// Icon-only small buttons (poster cards).
    Compact,
    /// A segmented row with icons and labels (inspector, detail page).
    #[default]
    Bar,
    /// Full-width 56 px buttons (the one-tap feedback page).
    Large,
}

/// Rating buttons; the stored choice is pressed. Disabled while saving.
#[component]
pub fn Choices<T>(
    choices: Vec<Choice<T>>,
    #[prop(into)] current: Signal<Option<T>>,
    #[prop(into)] saving: Signal<bool>,
    on_pick: Callback<T>,
    #[prop(optional)] style: ChoiceStyle,
    #[prop(into)] aria_label: String,
) -> impl IntoView
where
    T: Copy + PartialEq + Send + Sync + 'static,
{
    let (group_class, button_class) = match style {
        ChoiceStyle::Compact => ("choices compact", "btn ghost sm icon-only choice"),
        ChoiceStyle::Bar => ("seg choices-bar", "seg-btn choice"),
        ChoiceStyle::Large => ("choices large", "btn block choice"),
    };
    let buttons = choices
        .into_iter()
        .map(|choice| {
            let value = choice.value;
            let pressed = Memo::new(move |_| current.get() == Some(value));
            let compact = style == ChoiceStyle::Compact;
            view! {
                <button
                    type="button"
                    class=button_class
                    aria-pressed=move || pressed.get().to_string()
                    aria-label=compact.then_some(choice.label)
                    title=compact.then_some(choice.label)
                    disabled=move || saving.get()
                    on:click=move |_| on_pick.run(value)
                >
                    {choice_glyph(choice.glyph, style != ChoiceStyle::Large)}
                    {(!compact).then_some(view! { <span>{choice.label}</span> })}
                </button>
            }
        })
        .collect_view();
    view! {
        <div class=group_class role="group" aria-label=aria_label>
            {buttons}
        </div>
    }
}

/// Shape and word for a recommendation status. Only failures take a fault
/// hue; a fresh pick is informational.
pub fn rec_status_kind(status: RecommendationStatus) -> StatusKind {
    match status {
        RecommendationStatus::Notified => StatusKind::Info,
        RecommendationStatus::Watched => StatusKind::Ok,
        RecommendationStatus::Failed => StatusKind::Fault,
        RecommendationStatus::Pending
        | RecommendationStatus::Abandoned
        | RecommendationStatus::Ignored => StatusKind::Idle,
    }
}

const MEDIA_SECTIONS: [(&str, &str); 3] = [
    ("/media", "Movies & TV"),
    ("/podcasts", "Podcasts"),
    ("/pods", "PressPods"),
];

/// Phone-only `Movies & TV | Podcasts | PressPods` switch under the title of
/// the three Media tab pages (the tab lands on the last one used). Links,
/// not buttons; the current section comes from the path.
#[component]
pub fn MediaSwitch() -> impl IntoView {
    let path = use_path();
    view! {
        <nav class="seg media-switch only-phone" aria-label="Media sections">
            {MEDIA_SECTIONS
                .iter()
                .map(|(href, label)| {
                    let href = *href;
                    let current = Signal::derive(move || {
                        path.with(|p| p == href || p.starts_with(&format!("{href}/")))
                            .then(|| "page".to_owned())
                    });
                    view! {
                        <Link to=href class="seg-btn" aria_current=current>
                            {*label}
                        </Link>
                    }
                })
                .collect_view()}
        </nav>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_failures_take_the_fault_shape() {
        assert_eq!(
            rec_status_kind(RecommendationStatus::Failed),
            StatusKind::Fault
        );
        assert_eq!(
            rec_status_kind(RecommendationStatus::Notified),
            StatusKind::Info
        );
        assert_eq!(
            rec_status_kind(RecommendationStatus::Ignored),
            StatusKind::Idle
        );
    }

    #[test]
    fn media_choices_cover_every_rating() {
        let values: Vec<_> = media_choices().iter().map(|c| c.value).collect();
        assert_eq!(
            values,
            [
                RecommendationFeedback::GoodPick,
                RecommendationFeedback::NotForMe,
                RecommendationFeedback::AlreadyWatched,
            ]
        );
    }
}
