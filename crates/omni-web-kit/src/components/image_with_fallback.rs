//! Image that swaps to a placeholder when missing or broken
//! (`components/ImageWithFallback.tsx`).

use leptos::prelude::*;

#[component]
pub fn ImageWithFallback(
    #[prop(into)] src: Option<String>,
    #[prop(into)] alt: String,
    #[prop(into)] class: String,
    #[prop(into)] placeholder_class: String,
    #[prop(into)] placeholder: ViewFn,
    #[prop(optional)] lazy: bool,
) -> impl IntoView {
    let broken = RwSignal::new(false);
    let placeholder_classes = format!("{class} {placeholder_class}");
    move || match (&src, broken.get()) {
        (Some(src), false) => view! {
            <img
                class=class.clone()
                src=src.clone()
                alt=alt.clone()
                loading=lazy.then_some("lazy")
                on:error=move |_| broken.set(true)
            />
        }
        .into_any(),
        _ => view! { <div class=placeholder_classes.clone()>{placeholder.run()}</div> }.into_any(),
    }
}
