//! Incremental list reveal.

use leptos::prelude::*;

/// The visible slice of `items` and its controls.
pub struct ShowMore<T: Send + Sync + 'static> {
    pub visible: Memo<Vec<T>>,
    pub has_more: Memo<bool>,
    pub remaining: Memo<usize>,
    count: RwSignal<usize>,
    page_size: usize,
}

impl<T: Send + Sync + 'static> Clone for ShowMore<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: Send + Sync + 'static> Copy for ShowMore<T> {}

impl<T: Send + Sync + 'static> ShowMore<T> {
    /// Grow by another page.
    pub fn show_more(&self) {
        let page = self.page_size;
        self.count.update(|count| *count += page);
    }
}

/// Render `page_size` items up front and grow by a page per click;
/// `reset_key` changes collapse back to the first page.
pub fn use_show_more<T>(
    items: Signal<Vec<T>>,
    page_size: usize,
    reset_key: Signal<String>,
) -> ShowMore<T>
where
    T: Clone + PartialEq + Send + Sync + 'static,
{
    let count = RwSignal::new(page_size);
    Effect::new(move |_| {
        reset_key.track();
        count.set(page_size);
    });
    let visible =
        Memo::new(move |_| items.with(|items| items.iter().take(count.get()).cloned().collect()));
    let has_more = Memo::new(move |_| items.with(Vec::len) > count.get());
    let remaining = Memo::new(move |_| items.with(Vec::len).saturating_sub(count.get()));
    ShowMore {
        visible,
        has_more,
        remaining,
        count,
        page_size,
    }
}

#[component]
pub fn ShowMoreButton(
    #[prop(into)] remaining: Signal<usize>,
    on_click: Callback<()>,
) -> impl IntoView {
    view! {
        <button type="button" class="show-more-btn" on:click=move |_| on_click.run(())>
            "Show More"
            <span class="show-more-count">{move || remaining.get()}</span>
        </button>
    }
}
