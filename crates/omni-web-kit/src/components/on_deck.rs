//! "What else could I watch" strip; renders nothing
//! when empty.

use leptos::prelude::*;
use omni_api::common::encode_uri_component;
use omni_api::media::{MediaType, OnDeckItem};

use super::image_with_fallback::ImageWithFallback;
use crate::router::Link;

#[component]
fn OnDeckCard(item: OnDeckItem) -> impl IntoView {
    let poster = item
        .poster_path
        .as_ref()
        .map(|p| format!("https://image.tmdb.org/t/p/w500{p}"));
    let (badge_class, badge) = match item.media_type {
        MediaType::Tv => ("media-badge media-tv", "TV"),
        MediaType::Movie => ("media-badge media-movie", "Movie"),
    };
    view! {
        <Link
            class="ondeck-card"
            to=format!("/media/{}", encode_uri_component(&item.recommendation_id))
            title=item.title.clone()
        >
            <ImageWithFallback
                src=poster
                alt=format!("{} poster", item.title)
                class="ondeck-poster"
                placeholder_class="ondeck-poster-placeholder"
                lazy=true
                placeholder=|| view! {
                    <svg
                        width="20"
                        height="20"
                        viewBox="0 0 24 24"
                        fill="none"
                        stroke="currentColor"
                        stroke-width="1.5"
                        stroke-linecap="round"
                        stroke-linejoin="round"
                    >
                        <rect x="3" y="4" width="18" height="16" rx="2"></rect>
                        <path d="M3 9h18M7 4v5M12 4v5M17 4v5"></path>
                    </svg>
                }
            />
            <div class="ondeck-body">
                <div class="ondeck-title-row">
                    <span class="ondeck-title">{item.title.clone()}</span>
                    {item.year.map(|year| view! { <span class="ondeck-year">{format!(" ({})", crate::utils::js::number_string(year))}</span> })}
                </div>
                <span class=badge_class>{badge}</span>
                {item.why_for_user.clone().map(|why| view! { <p class="ondeck-why">{why}</p> })}
            </div>
        </Link>
    }
}

#[component]
pub fn OnDeck(#[prop(into)] items: Signal<Vec<OnDeckItem>>) -> impl IntoView {
    let has_items = Memo::new(move |_| items.with(|i| !i.is_empty()));
    move || {
        has_items.get().then(|| {
            view! {
                <section class="page-section ondeck-section">
                    <div class="section-heading-row">
                        <h2 class="section-title">"On Deck"</h2>
                        <Link to="/media" class="section-view-all">"View All Picks ›"</Link>
                    </div>
                    <div class="ondeck-row">
                        <For
                            each=move || items.get()
                            key=|item| (item.recommendation_id.clone(), format!("{item:?}"))
                            children=|item| view! { <OnDeckCard item/> }
                        />
                    </div>
                </section>
            }
        })
    }
}
