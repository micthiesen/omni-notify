//! [`Avatar`] monograms and [`Poster`] artwork.

use leptos::prelude::*;

use super::platform_icon::PlatformIcon;
use super::tone::hue_class;

/// Up to two initials ("Hutch" → "H", "Lofi Girl" → "LG").
pub fn initials(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().collect();
    let pick = |w: &str| w.chars().next().map(|c| c.to_uppercase().to_string());
    match words.as_slice() {
        [] => "?".to_owned(),
        [one] => pick(one).unwrap_or_default(),
        [first, .., last] => format!(
            "{}{}",
            pick(first).unwrap_or_default(),
            pick(last).unwrap_or_default()
        ),
    }
}

/// Avatar presence ring.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Presence {
    #[default]
    None,
    Live,
    Offline,
}

/// Monogram on a deterministic muted gradient (or an image), sizes 24 to 64.
#[component]
pub fn Avatar(
    #[prop(into)] name: String,
    /// Seed for the gradient (defaults to the name).
    #[prop(into, optional)]
    seed: Option<String>,
    #[prop(optional, default = 32)] size: u32,
    #[prop(into, optional)] presence: Signal<Presence>,
    #[prop(into, optional)] platform: Option<String>,
    #[prop(into, optional)] image: Option<String>,
) -> impl IntoView {
    let hue = hue_class(seed.as_deref().unwrap_or(&name));
    let class = move || {
        format!(
            "avatar {hue} {}",
            match presence.get() {
                Presence::None => "",
                Presence::Live => "live",
                Presence::Offline => "offline",
            }
        )
    };
    let face = match image {
        Some(src) => view! { <img src=src alt="" loading="lazy"/> }.into_any(),
        None => view! { <span aria-hidden="true">{initials(&name)}</span> }.into_any(),
    };
    view! {
        <span class=class style=format!("--av: {size}px") role="img" aria-label=name.clone()>
            {face}
            {(size >= 40).then_some(()).and(platform).map(|p| view! { <PlatformIcon platform=p/> })}
        </span>
    }
}

/// 2:3 poster (movies) or 1:1 artwork. Falls back to the title set on a
/// gradient derived from the title. `kind` is the small uppercase label.
#[component]
pub fn Poster(
    #[prop(into)] src: Option<String>,
    #[prop(into)] title: String,
    #[prop(into, optional)] kind: Option<String>,
    #[prop(optional)] square: bool,
    #[prop(optional)] eager: bool,
) -> impl IntoView {
    let broken = RwSignal::new(false);
    let hue = hue_class(&title);
    let alt = format!("{title} artwork");
    let class = if square { "poster square" } else { "poster" };
    view! {
        <span class=class>
            {move || match (&src, broken.get()) {
                (Some(src), false) => view! {
                    <img
                        src=src.clone()
                        alt=alt.clone()
                        loading=(!eager).then_some("lazy")
                        decoding="async"
                        on:error=move |_| broken.set(true)
                    />
                }
                .into_any(),
                _ => view! { <span class=format!("poster-fallback {hue}")>{title.clone()}</span> }.into_any(),
            }}
            {kind.map(|k| view! { <span class="poster-kind">{k}</span> })}
        </span>
    }
}

#[cfg(test)]
mod tests {
    use super::initials;

    #[test]
    fn initials_take_first_and_last_words() {
        assert_eq!(initials("Hutch"), "H");
        assert_eq!(initials("lofi girl radio"), "LR");
        assert_eq!(initials("  "), "?");
    }
}
