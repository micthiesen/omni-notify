//! Page structure: [`PageHead`], [`Section`], [`Panel`], [`PanelHead`],
//! [`Disclosure`].

use leptos::prelude::*;

use super::icon::{Glyph, Icon, IconSize};

/// Page title block. `sentence` renders the title as the display status
/// sentence (overview pages); `actions` hold at most one primary button.
#[component]
pub fn PageHead(
    #[prop(into)] title: Signal<String>,
    #[prop(into, optional)] eyebrow: MaybeProp<String>,
    #[prop(into, optional)] lede: MaybeProp<String>,
    #[prop(optional)] sentence: bool,
    #[prop(optional)] actions: Option<ViewFn>,
    /// Extra content under the title (meta rows, tags).
    #[prop(optional)]
    children: Option<Children>,
) -> impl IntoView {
    let heading = if sentence {
        view! { <h1 class="sentence">{move || title.get()}</h1> }.into_any()
    } else {
        view! { <h1 class="page-title">{move || title.get()}</h1> }.into_any()
    };
    view! {
        <header class="page-head">
            <div class="page-head-text">
                {move || eyebrow.get().map(|e| view! { <span class="eyebrow">{e}</span> })}
                {heading}
                {move || lede.get().map(|l| view! { <p class="lede">{l}</p> })}
                {children.map(|c| c())}
            </div>
            {actions.map(|a| view! { <div class="page-actions">{a.run()}</div> })}
        </header>
    }
}

/// A titled page section. `end` sits at the right of the heading (links,
/// segmented controls).
#[component]
pub fn Section(
    #[prop(into)] title: Signal<String>,
    #[prop(into, optional)] meta: MaybeProp<String>,
    #[prop(optional)] end: Option<ViewFn>,
    #[prop(into, optional)] id: MaybeProp<String>,
    #[prop(into, optional)] class: MaybeProp<String>,
    children: Children,
) -> impl IntoView {
    view! {
        <section
            class=move || format!("section {}", class.get().unwrap_or_default())
            id=move || id.get()
        >
            <div class="section-head">
                <h2 class="section-title">{move || title.get()}</h2>
                {move || meta.get().map(|m| view! { <span class="section-meta">{m}</span> })}
                {end.map(|e| view! { <div class="section-end">{e.run()}</div> })}
            </div>
            {children()}
        </section>
    }
}

/// Panel header: h3 title, optional mono meta, trailing content.
#[component]
pub fn PanelHead(
    #[prop(into)] title: Signal<String>,
    #[prop(optional)] lead: Option<ViewFn>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    view! {
        <div class="panel-head">
            <h3 class="panel-title">{lead.map(|l| l.run())} {move || title.get()}</h3>
            {children.map(|c| view! { <div class="panel-meta">{c()}</div> })}
        </div>
    }
}

/// A surface for a group of rows. `stage` adds the live glow (one per page,
/// only while live). `refreshing` dims stale content under a progress line.
#[component]
pub fn Panel(
    #[prop(into, optional)] title: MaybeProp<String>,
    #[prop(optional)] head_end: Option<ViewFn>,
    #[prop(optional)] pad: bool,
    #[prop(into, optional)] stage: Signal<bool>,
    #[prop(into, optional)] refreshing: Signal<bool>,
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(into, optional)] id: MaybeProp<String>,
    #[prop(into, optional)] aria_label: MaybeProp<String>,
    children: Children,
) -> impl IntoView {
    let class = move || {
        let mut c = String::from("panel");
        if stage.get() {
            c.push_str(" stage");
        }
        if refreshing.get() {
            c.push_str(" refreshing");
        }
        if let Some(extra) = class.get() {
            c.push(' ');
            c.push_str(&extra);
        }
        c
    };
    let head = title.get_untracked().map(|_| {
        let title = Signal::derive(move || title.get().unwrap_or_default());
        match head_end {
            Some(end) => view! { <PanelHead title>{end.run()}</PanelHead> }.into_any(),
            None => view! { <PanelHead title/> }.into_any(),
        }
    });
    view! {
        <section class=class id=move || id.get() aria-label=move || aria_label.get()>
            {move || refreshing.get().then(|| view! { <div class="progress-line" role="presentation"></div> })}
            {head}
            {if pad {
                view! { <div class="panel-body">{children()}</div> }.into_any()
            } else {
                children().into_any()
            }}
        </section>
    }
}

/// The one expand control. The summary says what and how many
/// ("Show 34 older streams"); content does not animate height.
#[component]
pub fn Disclosure(
    #[prop(into)] summary: Signal<String>,
    #[prop(into, optional)] meta: MaybeProp<String>,
    #[prop(optional)] open: bool,
    /// No horizontal padding (inside text blocks).
    #[prop(optional)]
    flush: bool,
    #[prop(into, optional)] id: MaybeProp<String>,
    #[prop(into, optional)] class: MaybeProp<String>,
    children: Children,
) -> impl IntoView {
    view! {
        <details
            class=move || format!("disc{} {}", if flush { " flush" } else { "" }, class.get().unwrap_or_default())
            open=open
            id=move || id.get()
        >
            <summary>
                <Glyph icon=Icon::ChevronRight size=IconSize::Small class="chev"/>
                <span>{move || summary.get()}</span>
                {move || meta.get().map(|m| view! { <span class="meta">{m}</span> })}
            </summary>
            <div class="disc-body">{children()}</div>
        </details>
    }
}
