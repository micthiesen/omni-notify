//! Selection and input controls: [`Segmented`], [`Chip`], [`SearchField`],
//! [`Kbd`].

use leptos::html::Input;
use leptos::prelude::*;

use super::icon::{Glyph, Icon, IconSize};

/// One option of a [`Segmented`] control.
#[derive(Clone, Debug, PartialEq)]
pub struct SegOption<T> {
    pub value: T,
    pub label: String,
    /// Inline mono count; zero-count options stay visible but dimmed.
    pub count: Option<usize>,
}

impl<T> SegOption<T> {
    pub fn new(value: T, label: impl Into<String>) -> Self {
        Self {
            value,
            label: label.into(),
            count: None,
        }
    }

    pub fn with_count(mut self, count: usize) -> Self {
        self.count = Some(count);
        self
    }
}

/// Single-select segmented control (ranges, modes, filters with counts).
#[component]
pub fn Segmented<T>(
    #[prop(into)] options: Signal<Vec<SegOption<T>>>,
    #[prop(into)] value: Signal<T>,
    on_change: Callback<T>,
    #[prop(into)] aria_label: String,
    #[prop(optional)] small: bool,
) -> impl IntoView
where
    T: Clone + PartialEq + Send + Sync + 'static,
{
    let buttons = move || {
        options
            .get()
            .into_iter()
            .map(|option| {
                let mine = option.value.clone();
                let pressed = Memo::new(move |_| value.with(|v| *v == mine));
                let zero = option.count == Some(0);
                let chosen = option.value.clone();
                view! {
                    <button
                        type="button"
                        class=if zero { "seg-btn zero" } else { "seg-btn" }
                        aria-pressed=move || pressed.get().to_string()
                        on:click=move |_| on_change.run(chosen.clone())
                    >
                        {option.label.clone()}
                        {option.count.map(|n| view! { <span class="seg-n">{n}</span> })}
                    </button>
                }
            })
            .collect_view()
    };
    view! {
        <div class=if small { "seg sm" } else { "seg" } role="group" aria-label=aria_label>
            {buttons}
        </div>
    }
}

/// Toggle chip for multi-select filters and tags.
#[component]
pub fn Chip(
    #[prop(into)] pressed: Signal<bool>,
    on_click: Callback<()>,
    #[prop(into, optional)] count: MaybeProp<usize>,
    #[prop(into, optional)] title: MaybeProp<String>,
    children: Children,
) -> impl IntoView {
    view! {
        <button
            type="button"
            class="chip"
            class:empty=move || count.get() == Some(0)
            aria-pressed=move || pressed.get().to_string()
            title=move || title.get()
            on:click=move |_| on_click.run(())
        >
            {children()}
            {move || count.get().map(|n| view! { <span class="seg-n">{n}</span> })}
        </button>
    }
}

/// A filter input with a search glyph. `/` focuses the first one on the page
/// (handled by the shell through `data-page-filter`).
#[component]
pub fn SearchField(
    #[prop(into)] value: Signal<String>,
    on_input: Callback<String>,
    #[prop(into)] placeholder: String,
    #[prop(into)] aria_label: String,
    #[prop(optional)] node_ref: Option<NodeRef<Input>>,
    /// Show the `/` hint.
    #[prop(optional)]
    shortcut: bool,
) -> impl IntoView {
    let node_ref = node_ref.unwrap_or_default();
    view! {
        <label class="search-field">
            <Glyph icon=Icon::Search size=IconSize::Small/>
            <input
                node_ref=node_ref
                class="input"
                type="search"
                data-page-filter="true"
                placeholder=placeholder
                aria-label=aria_label
                prop:value=move || value.get()
                on:input=move |ev| on_input.run(event_target_value(&ev))
                on:keydown=move |ev: web_sys::KeyboardEvent| {
                    if ev.key() == "Escape" && !value.get_untracked().is_empty() {
                        ev.stop_propagation();
                        on_input.run(String::new());
                    }
                }
            />
            {shortcut.then(|| view! { <span class="kbd hide-phone" aria-hidden="true">"/"</span> })}
        </label>
    }
}

/// A keyboard key.
#[component]
pub fn Kbd(children: Children) -> impl IntoView {
    view! { <kbd class="kbd">{children()}</kbd> }
}
