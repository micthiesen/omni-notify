//! The sticky top bar (crumbs, page actions slot, wide-screen clock; on
//! phone the page title, connection dot and search) and the phone tab bar.

use leptos::prelude::*;
use omni_web_kit::components::{Glyph, Icon, IconSize};
use omni_web_kit::hooks::{store_pref, stored_pref, use_now};
use omni_web_kit::router::Link;
use omni_web_kit::utils::js::local_time_parts;

use super::ShellContext;
use super::connection::Connection;
use super::nav::Crumb;

const MEDIA_PREF: &str = "omni.media-tab";
const MEDIA_PATHS: [&str; 3] = ["/media", "/podcasts", "/pods"];

/// The Media tab target: the last used of `/media`, `/podcasts`, `/pods`.
pub fn media_target(stored: Option<&str>) -> &'static str {
    stored
        .and_then(|s| MEDIA_PATHS.iter().find(|p| **p == s))
        .copied()
        .unwrap_or("/media")
}

/// The Media section a path belongs to, if any.
pub fn media_section(path: &str) -> Option<&'static str> {
    MEDIA_PATHS
        .iter()
        .find(|p| path == **p || path.starts_with(&format!("{p}/")))
        .copied()
}

fn timezone_abbr() -> String {
    let date = js_sys::Date::new_0();
    let options = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&options, &"timeZoneName".into(), &"short".into());
    let text: String = date
        .to_locale_time_string_with_options("en-US", &options)
        .into();
    text.rsplit(' ').next().unwrap_or_default().to_owned()
}

#[component]
fn Clock() -> impl IntoView {
    let now = use_now(1000);
    let zone = timezone_abbr();
    view! {
        <span class="topbar-clock num" aria-hidden="true">
            {move || {
                let (h, m, _, _) = local_time_parts(now.get());
                format!("{h:02}:{m:02} {zone}")
            }}
        </span>
    }
}

#[component]
pub fn TopBar(shell: ShellContext, #[prop(into)] crumbs: Signal<Vec<Crumb>>) -> impl IntoView {
    view! {
        <header class="topbar">
            <nav class="crumbs" aria-label="Breadcrumb">
                {move || {
                    let crumbs = crumbs.get();
                    let last = crumbs.len().saturating_sub(1);
                    crumbs
                        .into_iter()
                        .enumerate()
                        .map(|(i, crumb)| {
                            let sep = (i > 0).then(|| view! { <span class="sep" aria-hidden="true">"/"</span> });
                            let node = match crumb.href {
                                Some(href) if i < last => view! { <Link to=href>{crumb.label}</Link> }.into_any(),
                                _ => view! { <span class="here" aria-current="page">{crumb.label}</span> }.into_any(),
                            };
                            view! { {sep} {node} }
                        })
                        .collect_view()
                }}
            </nav>
            <span class="spacer"></span>
            <Clock/>
            <div class="topbar-phone">
                <Connection compact=true/>
                <button
                    type="button"
                    class="btn ghost icon-only"
                    aria-label="Search"
                    on:click=move |_| shell.palette_open.set(true)
                >
                    <Glyph icon=Icon::Search/>
                </button>
            </div>
        </header>
    }
}

#[component]
pub fn TabBar(
    shell: ShellContext,
    #[prop(into)] path: Signal<String>,
    #[prop(into)] live_count: Signal<usize>,
    #[prop(into)] faults: Signal<usize>,
) -> impl IntoView {
    let media = RwSignal::new(media_target(stored_pref(MEDIA_PREF).as_deref()));
    Effect::new(move |_| {
        if let Some(section) = path.with(|p| media_section(p)) {
            media.set(section);
            store_pref(MEDIA_PREF, section);
        }
    });
    let current = move |section: &'static str| {
        path.with(|p| match section {
            "home" => p == "/",
            "live" => p == "/live" || p.starts_with("/streamers/"),
            "media" => media_section(p).is_some() || p.starts_with("/feedback/"),
            "research" => p.starts_with("/workspaces") || p == "/briefings",
            _ => false,
        })
    };
    let aria = move |section: &'static str| {
        Signal::derive(move || current(section).then(|| "page".to_owned()))
    };
    view! {
        <nav class="tabbar" aria-label="Sections">
            <Link to="/" class="tab" aria_current=aria("home")>
                <Glyph icon=Icon::Home/>
                "Home"
            </Link>
            <Link to="/live" class="tab" aria_current=aria("live")>
                <Glyph icon=Icon::Live/>
                "Live"
                {move || {
                    let n = live_count.get();
                    (n > 0).then(|| view! { <span class="badge live" aria-label=format!("{n} live")>{n}</span> })
                }}
            </Link>
            {move || {
                let to = media.get();
                view! {
                    <Link to=to class="tab" aria_current=aria("media")>
                        <Glyph icon=Icon::Film/>
                        "Media"
                    </Link>
                }
            }}
            <Link to="/workspaces" class="tab" aria_current=aria("research")>
                <Glyph icon=Icon::Flask/>
                "Research"
            </Link>
            <button
                type="button"
                class="tab"
                aria-haspopup="dialog"
                aria-expanded=move || shell.palette_open.get().to_string()
                aria-controls="nav-palette"
                on:click=move |_| shell.palette_open.set(true)
            >
                <Glyph icon=Icon::Grid size=IconSize::Medium/>
                "Go"
                {move || {
                    let n = faults.get();
                    (n > 0).then(|| view! { <span class="badge fault" aria-label=format!("{n} failing")>{n}</span> })
                }}
            </button>
        </nav>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_tab_remembers_its_section() {
        assert_eq!(media_target(Some("/pods")), "/pods");
        assert_eq!(media_target(Some("/evil")), "/media");
        assert_eq!(media_target(None), "/media");
        assert_eq!(media_section("/podcasts/abc"), Some("/podcasts"));
        assert_eq!(media_section("/podsx"), None);
    }
}
