//! Status filter chip bar. Statuses with
//! zero items are hidden; clicking the active chip clears it.

use std::collections::HashMap;

use leptos::prelude::*;

/// `order` is `(key, label)`; the empty key means "All".
#[component]
pub fn StatusFilterChips(
    order: Vec<(String, String)>,
    #[prop(into)] counts: Signal<HashMap<String, usize>>,
    #[prop(into)] total: Signal<usize>,
    #[prop(into)] active: Signal<String>,
    on_change: Callback<String>,
) -> impl IntoView {
    let chips = move || {
        let counts = counts.get();
        order
            .iter()
            .filter(|(key, _)| counts.contains_key(key))
            .map(|(key, label)| {
                let key = key.clone();
                let count = counts.get(&key).copied().unwrap_or(0);
                let is_active = {
                    let key = key.clone();
                    move || active.get() == key
                };
                let pressed = is_active.clone();
                let class_active = is_active.clone();
                view! {
                    <button
                        type="button"
                        class=move || {
                            format!("chip-btn {}", if class_active() { "active" } else { "" })
                        }
                        aria-pressed=move || pressed().to_string()
                        on:click=move |_| {
                            on_change.run(if is_active() { String::new() } else { key.clone() })
                        }
                    >
                        {label.clone()}
                        " "
                        <span class="chip-btn-count">{count}</span>
                    </button>
                }
            })
            .collect_view()
    };
    view! {
        <div class="rec-filters" role="group" aria-label="Filter by Status">
            <button
                type="button"
                class=move || format!("chip-btn {}", if active.get().is_empty() { "active" } else { "" })
                aria-pressed=move || active.get().is_empty().to_string()
                on:click=move |_| on_change.run(String::new())
            >
                "All "
                <span class="chip-btn-count">{move || total.get()}</span>
            </button>
            {chips}
        </div>
    }
}
