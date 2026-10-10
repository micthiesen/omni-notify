//! Loading, empty and error states.

use leptos::prelude::*;

use super::button::{Button, ButtonSize};
use super::icon::{Glyph, Icon};
use super::surface::Disclosure;
use crate::router::Link;

/// Skeleton shapes; geometry matches the final layout.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SkeletonKind {
    #[default]
    Line,
    Title,
    Readout,
    Poster,
}

#[component]
pub fn Skeleton(
    #[prop(optional)] kind: SkeletonKind,
    /// CSS width (`"60%"`, `"120px"`).
    #[prop(into, optional)]
    width: Option<String>,
) -> impl IntoView {
    let class = match kind {
        SkeletonKind::Line => "skel",
        SkeletonKind::Title => "skel title",
        SkeletonKind::Readout => "skel readout",
        SkeletonKind::Poster => "skel poster",
    };
    view! { <span class=class style=width.map(|w| format!("width: {w}")) aria-hidden="true"></span> }
}

/// `count` skeleton rows at real row height.
#[component]
pub fn SkeletonRows(
    #[prop(optional, default = 5)] count: usize,
    #[prop(into, optional)] label: MaybeProp<String>,
) -> impl IntoView {
    view! {
        <div class="rows skel-rows" role="status" aria-label=move || label.get().unwrap_or_else(|| "Loading".to_owned())>
            {(0..count)
                .map(|i| {
                    let width = format!("{}%", 40 + (i * 17) % 45);
                    view! {
                        <div class="row">
                            <Skeleton width=width/>
                            <Skeleton width="15%"/>
                        </div>
                    }
                })
                .collect_view()}
        </div>
    }
}

/// One sentence and at most one action. `title` adds a heading (page-level
/// empty and setup states): the glyph sits beside the heading and sentence.
#[component]
pub fn EmptyState(
    #[prop(into)] message: Signal<String>,
    #[prop(into, optional)] title: MaybeProp<String>,
    #[prop(optional)] icon: Option<Icon>,
    #[prop(optional)] compact: bool,
    #[prop(optional)] action: Option<ViewFn>,
) -> impl IntoView {
    let glyph = icon.map(|icon| view! { <span class="empty-glyph"><Glyph icon/></span> });
    if let Some(heading) = title.get_untracked() {
        return view! {
            <div class="empty titled">
                {glyph}
                <div class="empty-body">
                    <h2 class="empty-title">{heading}</h2>
                    <p>{move || message.get()}</p>
                    {action.map(|a| view! { <div class="empty-action">{a.run()}</div> })}
                </div>
            </div>
        }
        .into_any();
    }
    view! {
        <div class=if compact { "empty compact" } else { "empty" }>
            {glyph}
            <p>{move || message.get()}</p>
            {action.map(|a| a.run())}
        </div>
    }
    .into_any()
}

/// What failed, why if known, Retry and an optional link. The raw API detail
/// goes into a disclosure, never the headline. `page` centers it as the page
/// body; `warn` uses the warning hue (configuration notices).
#[component]
pub fn ErrorState(
    #[prop(into)] title: Signal<String>,
    #[prop(into, optional)] detail: MaybeProp<String>,
    #[prop(into, optional)] raw: MaybeProp<String>,
    #[prop(optional)] retry: Option<Callback<()>>,
    /// `(label, href)` of a helpful page (Operations, setup).
    #[prop(optional)]
    link: Option<(String, String)>,
    #[prop(optional)] page: bool,
    #[prop(optional)] warn: bool,
) -> impl IntoView {
    let class = format!(
        "error-state{}{}",
        if page { " page" } else { "" },
        if warn { " warn" } else { "" }
    );
    view! {
        <div class=class role="alert">
            <span class=if warn { "status warn dot-only" } else { "status fault dot-only" } aria-hidden="true"></span>
            <div class="error-state-body">
                <p class="error-state-title">{move || title.get()}</p>
                {move || detail.get().map(|d| view! { <p class="error-state-detail">{d}</p> })}
                {move || raw.get().filter(|r| !r.is_empty()).map(|r| view! {
                    <Disclosure summary="Details" flush=true>
                        <pre>{r}</pre>
                    </Disclosure>
                })}
                {(retry.is_some() || link.is_some()).then(|| view! {
                    <div class="cluster">
                        {retry.map(|retry| view! {
                            <Button size=ButtonSize::Sm icon=Icon::Refresh on_click=Callback::new(move |_| retry.run(()))>
                                "Retry"
                            </Button>
                        })}
                        {link.map(|(label, href)| view! { <Link to=href class="textlink">{label}</Link> })}
                    </div>
                })}
            </div>
        </div>
    }
}

/// A one-line note (freshness, "updated 3m ago", partial failures).
#[component]
pub fn InlineNote(
    #[prop(optional)] tone: super::tone::Tone,
    #[prop(into, optional)] role: Option<&'static str>,
    children: Children,
) -> impl IntoView {
    view! { <p class=format!("inline-note {}", tone.class()) role=role>{children()}</p> }
}
