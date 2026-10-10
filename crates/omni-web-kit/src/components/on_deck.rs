//! "What else could I watch": a poster rail of recommendation picks.
//! Renders nothing when empty.

use leptos::prelude::*;
use omni_api::common::encode_uri_component;
use omni_api::media::{MediaType, OnDeckItem};

use super::avatar::Poster;
use crate::router::Link;
use crate::utils::js::number_string;

/// TMDB w500 poster URL.
pub fn tmdb_poster(path: Option<&str>) -> Option<String> {
    path.map(|p| format!("https://image.tmdb.org/t/p/w500{p}"))
}

#[component]
pub fn PosterCard(item: OnDeckItem) -> impl IntoView {
    let kind = match item.media_type {
        MediaType::Tv => "TV",
        MediaType::Movie => "Movie",
    };
    let title = match item.year {
        Some(year) => format!("{} ({})", item.title, number_string(year)),
        None => item.title.clone(),
    };
    view! {
        <Link
            class="poster-card"
            to=format!("/media/{}", encode_uri_component(&item.recommendation_id))
            title=item.title.clone()
        >
            <Poster src=tmdb_poster(item.poster_path.as_deref()) title=item.title.clone() kind=kind captioned=true/>
            <span class="poster-card-title">{title}</span>
            {item.why_for_user.clone().map(|why| view! { <span class="poster-card-why">{why}</span> })}
        </Link>
    }
}

/// The poster rail; scroll-snaps on phone.
#[component]
pub fn OnDeck(#[prop(into)] items: Signal<Vec<OnDeckItem>>) -> impl IntoView {
    view! {
        <div class="poster-rail">
            <For
                each=move || items.get()
                key=|item| (item.recommendation_id.clone(), format!("{item:?}"))
                children=|item| view! { <PosterCard item/> }
            />
        </div>
    }
}
