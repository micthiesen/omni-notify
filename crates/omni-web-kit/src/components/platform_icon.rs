//! YouTube / Twitch / Kick glyphs, filled from the platform tokens.

use leptos::prelude::*;

fn platform_path(platform: &str) -> Option<&'static str> {
    match platform {
        "youtube" => Some(
            "M23.5 6.2a3 3 0 0 0-2.1-2.1C19.5 3.5 12 3.5 12 3.5s-7.5 0-9.4.6A3 3 0 0 0 .5 6.2 31.3 31.3 0 0 0 0 12a31.3 31.3 0 0 0 .5 5.8 3 3 0 0 0 2.1 2.1c1.9.6 9.4.6 9.4.6s7.5 0 9.4-.6a3 3 0 0 0 2.1-2.1A31.3 31.3 0 0 0 24 12a31.3 31.3 0 0 0-.5-5.8zM9.6 15.6V8.4L15.8 12z",
        ),
        "twitch" => Some(
            "M2.1 0 .5 4.3v17.1h5.9V24h3.2l2.7-2.6h4.8L23.5 15V0zm19.3 13.9-3.7 3.7h-5.9l-2.7 2.6v-2.6H4.3V2.1h17.1zM17.1 6.2h-2.2v6.4h2.2zm-5.8 0H9.1v6.4h2.2z",
        ),
        "kick" => {
            Some("M3 2h6.5v5.5H12V5h2.5V2H21v7h-2.5v2H16v2h2.5v2H21v7h-6.5v-3H12v-2.5H9.5V22H3z")
        }
        _ => None,
    }
}

/// Nothing for unknown platforms.
#[component]
pub fn PlatformIcon(
    #[prop(into)] platform: String,
    #[prop(default = 16)] size: u32,
) -> impl IntoView {
    platform_path(&platform).map(|path| {
        view! {
            <svg
                width=size
                height=size
                viewBox="0 0 24 24"
                aria-label=platform.clone()
                role="img"
                class=format!("platform-icon {platform}")
            >
                <path d=path></path>
            </svg>
        }
    })
}
