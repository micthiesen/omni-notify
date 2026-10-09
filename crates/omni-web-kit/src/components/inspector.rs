//! Floating detail layers: [`Inspector`] (drawer on desk, bottom sheet on
//! phone, docked pane on wide pages that opt in) and [`Modal`] (full-screen
//! log viewer, Data JSON on phone). Both use [`use_modal`]: focus trap,
//! Escape, scroll lock and focus restore; `.inspector-close` is focused first.

use leptos::prelude::*;

use super::icon::{Glyph, Icon};
use crate::hooks::use_modal;

#[component]
fn CloseButton(on_close: Callback<()>) -> impl IntoView {
    view! {
        <button
            type="button"
            class="btn ghost icon-only inspector-close"
            aria-label="Close"
            on:click=move |_| on_close.run(())
        >
            <Glyph icon=Icon::Close/>
        </button>
    }
}

#[component]
fn InspectorHead(
    title: Signal<String>,
    status: Option<ViewFn>,
    on_close: Callback<()>,
) -> impl IntoView {
    view! {
        <span class="grab" aria-hidden="true"></span>
        <div class="inspector-head">
            <div class="inspector-title">
                {status.map(|s| view! { <div class="cluster">{s.run()}</div> })}
                <h2>{move || title.get()}</h2>
            </div>
            <CloseButton on_close/>
        </div>
    }
}

#[component]
fn DrawerInspector(
    title: Signal<String>,
    status: Option<ViewFn>,
    actions: Option<ViewFn>,
    on_close: Callback<()>,
    wide: bool,
    children: ChildrenFn,
) -> impl IntoView {
    let panel = use_modal(move || on_close.run(()));
    view! {
        <button
            type="button"
            class="scrim"
            tabindex="-1"
            aria-label="Close"
            on:click=move |_| on_close.run(())
        ></button>
        <div
            node_ref=panel
            class=if wide { "inspector wide" } else { "inspector" }
            role="dialog"
            aria-modal="true"
            aria-label=move || title.get()
            tabindex="-1"
        >
            <InspectorHead title status on_close/>
            {actions.map(|a| view! { <div class="inspector-actions">{a.run()}</div> })}
            <div class="inspector-body">{children()}</div>
        </div>
    }
}

/// Right drawer (440 px; 720 with `wide`), phone bottom sheet, or with
/// `docked` an in-flow pane for `.split.docked` layouts on wide screens.
/// Content goes in `.inspector-section` blocks.
#[component]
pub fn Inspector(
    #[prop(into)] title: Signal<String>,
    #[prop(optional)] status: Option<ViewFn>,
    #[prop(optional)] actions: Option<ViewFn>,
    on_close: Callback<()>,
    #[prop(into, optional)] docked: Signal<bool>,
    #[prop(optional)] wide: bool,
    children: ChildrenFn,
) -> impl IntoView {
    move || {
        let status = status.clone();
        let actions = actions.clone();
        let children = children.clone();
        if docked.get() {
            view! {
                <aside class="inspector docked" aria-label=move || title.get()>
                    <InspectorHead title status on_close/>
                    {actions.map(|a| view! { <div class="inspector-actions">{a.run()}</div> })}
                    <div class="inspector-body">{children()}</div>
                </aside>
            }
            .into_any()
        } else {
            view! { <DrawerInspector title status actions on_close wide children/> }.into_any()
        }
    }
}

/// Full-screen modal (inset on desk). `head` renders beside the close button.
#[component]
pub fn Modal(
    #[prop(into)] label: Signal<String>,
    on_close: Callback<()>,
    #[prop(optional)] head: Option<ViewFn>,
    children: Children,
) -> impl IntoView {
    let panel = use_modal(move || on_close.run(()));
    view! {
        <div
            node_ref=panel
            class="modal"
            role="dialog"
            aria-modal="true"
            aria-label=move || label.get()
            tabindex="-1"
        >
            <div class="inspector-head">
                <div class="inspector-title">{head.map(|h| h.run())}</div>
                <CloseButton on_close/>
            </div>
            {children()}
        </div>
        <button
            type="button"
            class="scrim over"
            tabindex="-1"
            aria-label="Close"
            on:click=move |_| on_close.run(())
        ></button>
    }
}
