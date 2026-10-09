//! Sidebar, mobile header, bottom navigation and "More" sheet
//! (`components/NavBar.tsx`).

use leptos::html::Div;
use leptos::prelude::*;
use omni_api::runs::RunStatus;
use omni_api::workspaces::WorkspaceOverview;
use wasm_bindgen::JsCast as _;
use wasm_bindgen::closure::Closure;

use crate::api;
use crate::live::{ConnectionState, use_live_data};
use crate::router::Link;
use crate::task::{on_cleanup_local, spawn_scoped};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Icon {
    Home,
    Watch,
    Listen,
    Research,
    More,
    Operations,
    Email,
    Pets,
    Costs,
    Data,
    Claude,
    Mcp,
}

#[derive(Clone, Copy)]
struct NavItem {
    to: &'static str,
    label: &'static str,
    icon: Icon,
    paths: &'static [&'static str],
}

const PRIMARY_LINKS: [NavItem; 4] = [
    NavItem {
        to: "/",
        label: "Home",
        icon: Icon::Home,
        paths: &[],
    },
    NavItem {
        to: "/media",
        label: "Watch",
        icon: Icon::Watch,
        paths: &["/media", "/streamers"],
    },
    NavItem {
        to: "/podcasts",
        label: "Listen",
        icon: Icon::Listen,
        paths: &["/podcasts", "/pods"],
    },
    NavItem {
        to: "/workspaces",
        label: "Research",
        icon: Icon::Research,
        paths: &["/workspaces", "/briefings"],
    },
];

const MORE_LINKS: [NavItem; 8] = [
    NavItem {
        to: "/reminders",
        label: "Reminders",
        icon: Icon::Operations,
        paths: &[],
    },
    NavItem {
        to: "/operations",
        label: "Operations",
        icon: Icon::Operations,
        paths: &[],
    },
    NavItem {
        to: "/emails",
        label: "Email",
        icon: Icon::Email,
        paths: &[],
    },
    NavItem {
        to: "/pets",
        label: "Pets",
        icon: Icon::Pets,
        paths: &[],
    },
    NavItem {
        to: "/costs",
        label: "Costs",
        icon: Icon::Costs,
        paths: &[],
    },
    NavItem {
        to: "/data",
        label: "Data",
        icon: Icon::Data,
        paths: &[],
    },
    NavItem {
        to: "/claude",
        label: "Claude",
        icon: Icon::Claude,
        paths: &[],
    },
    NavItem {
        to: "/mcp-activity",
        label: "MCP",
        icon: Icon::Mcp,
        paths: &[],
    },
];

fn is_path_active(path: &str, item: &NavItem) -> bool {
    let own = [item.to];
    let candidates: &[&str] = if item.paths.is_empty() {
        &own
    } else {
        item.paths
    };
    candidates.iter().any(|candidate| {
        if *candidate == "/" {
            path == "/"
        } else {
            path == *candidate || path.starts_with(&format!("{candidate}/"))
        }
    })
}

#[component]
fn NavIcon(icon: Icon) -> impl IntoView {
    let shapes = match icon {
        Icon::Home => view! {
            <path d="m3 11 9-8 9 8"></path>
            <path d="M5 10v10h14V10"></path>
            <path d="M9 20v-6h6v6"></path>
        }
        .into_any(),
        Icon::Watch => view! {
            <rect x="3" y="5" width="18" height="14" rx="2"></rect>
            <path d="m10 9 5 3-5 3Z"></path>
        }
        .into_any(),
        Icon::Listen => view! {
            <path d="M4 13a8 8 0 0 1 16 0"></path>
            <path d="M4 13v5a2 2 0 0 0 2 2h2v-8H4"></path>
            <path d="M20 13v5a2 2 0 0 1-2 2h-2v-8h4"></path>
        }
        .into_any(),
        Icon::Research => view! {
            <path d="M4 5.5A2.5 2.5 0 0 1 6.5 3H11v16H6.5A2.5 2.5 0 0 0 4 21.5Z"></path>
            <path d="M20 5.5A2.5 2.5 0 0 0 17.5 3H13v16h4.5a2.5 2.5 0 0 1 2.5 2.5Z"></path>
        }
        .into_any(),
        Icon::More => view! {
            <circle cx="5" cy="12" r="1" fill="currentColor"></circle>
            <circle cx="12" cy="12" r="1" fill="currentColor"></circle>
            <circle cx="19" cy="12" r="1" fill="currentColor"></circle>
        }
        .into_any(),
        Icon::Operations => view! {
            <path d="M4 6h16M4 12h16M4 18h16"></path>
            <circle cx="8" cy="6" r="2" fill="var(--bg-card)"></circle>
            <circle cx="16" cy="12" r="2" fill="var(--bg-card)"></circle>
            <circle cx="10" cy="18" r="2" fill="var(--bg-card)"></circle>
        }
        .into_any(),
        Icon::Email => view! {
            <rect x="3" y="5" width="18" height="14" rx="2"></rect>
            <path d="m4 7 8 6 8-6"></path>
        }
        .into_any(),
        Icon::Pets => view! {
            <circle cx="8" cy="8" r="2"></circle>
            <circle cx="16" cy="8" r="2"></circle>
            <circle cx="5" cy="13" r="2"></circle>
            <circle cx="19" cy="13" r="2"></circle>
            <path d="M8 18c0-3 2-5 4-5s4 2 4 5c0 2-2 3-4 3s-4-1-4-3Z"></path>
        }
        .into_any(),
        Icon::Costs => view! {
            <circle cx="12" cy="12" r="9"></circle>
            <path d="M15 8.5c-.8-.6-1.8-1-3-1-1.7 0-3 1-3 2.3 0 3.7 6 1.4 6 5.2 0 1.4-1.3 2.5-3.2 2.5-1.2 0-2.4-.4-3.3-1.2M12 5v14"></path>
        }
        .into_any(),
        Icon::Data => view! {
            <ellipse cx="12" cy="5" rx="8" ry="3"></ellipse>
            <path d="M4 5v7c0 1.7 3.6 3 8 3s8-1.3 8-3V5"></path>
            <path d="M4 12v7c0 1.7 3.6 3 8 3s8-1.3 8-3v-7"></path>
        }
        .into_any(),
        Icon::Claude => view! {
            <rect x="3" y="4" width="18" height="16" rx="2"></rect>
            <path d="m7 9 3 3-3 3"></path>
            <path d="M13 15h4"></path>
        }
        .into_any(),
        Icon::Mcp => view! {
            <circle cx="6" cy="12" r="2.5"></circle>
            <circle cx="18" cy="6" r="2.5"></circle>
            <circle cx="18" cy="18" r="2.5"></circle>
            <path d="M8.3 10.9 15.7 7.1M8.3 13.1l7.4 3.8"></path>
        }
        .into_any(),
    };
    view! {
        <svg
            class="nav-icon"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="1.7"
            stroke-linecap="round"
            stroke-linejoin="round"
            aria-hidden="true"
        >
            {shapes}
        </svg>
    }
}

#[component]
fn ConnectionBadge() -> impl IntoView {
    let live = use_live_data();
    let label = move || match live.connection.get() {
        ConnectionState::Connecting => "Connecting",
        ConnectionState::Live => "Live",
        ConnectionState::Polling => "Reconnecting",
    };
    let title = move || {
        let detail = match live.connection.get() {
            ConnectionState::Connecting => "Establishing realtime connection…",
            ConnectionState::Live => "Realtime updates connected",
            ConnectionState::Polling => "Realtime stream down, polling every 10s",
        };
        format!("{detail} (click to refresh)")
    };
    view! {
        <button
            type="button"
            class=move || format!("conn-badge conn-{}", live.connection.get().as_str())
            title=title
            on:click=move |_| {
                let _ = window().location().reload();
            }
        >
            <span class="conn-dot"></span>
            {label}
        </button>
    }
}

#[component]
fn Brand() -> impl IntoView {
    view! {
        <Link to="/" class="nav-brand">
            <svg width="19" height="19" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
                <path d="M12 2a7 7 0 0 0-7 7v3.3l-1.7 2.7A1.5 1.5 0 0 0 4.6 17.3h14.8a1.5 1.5 0 0 0 1.3-2.3L19 12.3V9a7 7 0 0 0-7-7Zm-2.5 16.3a2.5 2.5 0 0 0 5 0Z"></path>
            </svg>
            <span>"Omni Notify"</span>
        </Link>
    }
}

/// `null` renders "!" (unknown), zero renders nothing.
#[component]
fn ActionBadge(
    #[prop(into)] value: Signal<Option<usize>>,
    #[prop(optional)] unknown_title: Option<&'static str>,
) -> impl IntoView {
    move || {
        match value.get() {
        Some(0) => None,
        Some(count) => Some(
            view! {
                <span class="nav-action-badge ">
                    {if count > 99 { "99+".to_owned() } else { count.to_string() }}
                </span>
            }
            .into_any(),
        ),
        None => Some(
            view! { <span class="nav-action-badge nav-action-unknown" title=unknown_title>"!"</span> }
                .into_any(),
        ),
    }
    }
}

fn nav_link(
    item: NavItem,
    mobile: bool,
    path: Signal<String>,
    research_actions: Signal<Option<usize>>,
    failing_tasks: Signal<Option<usize>>,
) -> impl IntoView {
    let base = if mobile {
        "mobile-nav-link"
    } else {
        "sidebar-link"
    };
    let class = Signal::derive(move || {
        format!(
            "{base} {}",
            if is_path_active(&path.get(), &item) {
                "active"
            } else {
                ""
            }
        )
    });
    let current =
        Signal::derive(move || is_path_active(&path.get(), &item).then(|| "page".to_owned()));
    let badge = match item.icon {
        Icon::Research => Some(
            view! { <ActionBadge value=research_actions unknown_title="Research status unavailable"/> }
                .into_any(),
        ),
        Icon::Operations => Some(view! { <ActionBadge value=failing_tasks/> }.into_any()),
        _ => None,
    };
    view! {
        <Link to=item.to class=class aria_current=current>
            <NavIcon icon=item.icon/>
            <span>{item.label}</span>
            {badge}
        </Link>
    }
}

/// The More sheet; mounted only while open. Locks scroll, traps Tab, closes
/// on Escape and restores focus on close.
#[component]
fn MoreSheet(
    path: Signal<String>,
    research_actions: Signal<Option<usize>>,
    failing_tasks: Signal<Option<usize>>,
    on_close: Callback<()>,
) -> impl IntoView {
    let sheet_ref = NodeRef::<Div>::new();
    let installed = StoredValue::new(false);
    Effect::new(move |_| {
        let Some(sheet) = sheet_ref.get() else { return };
        if installed.get_value() {
            return;
        }
        installed.set_value(true);
        let sheet: web_sys::Element = sheet.into();
        let doc = document();
        let previously_focused = doc
            .active_element()
            .and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok());
        let body = doc.body();
        let previous_overflow = body
            .as_ref()
            .and_then(|b| b.style().get_property_value("overflow").ok())
            .unwrap_or_default();
        if let Some(body) = &body {
            let _ = body.style().set_property("overflow", "hidden");
        }
        if let Some(close) = sheet
            .query_selector(".mobile-more-close")
            .ok()
            .flatten()
            .and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok())
        {
            let _ = close.focus();
        }
        let key_sheet = sheet.clone();
        let on_key = Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(
            move |event: web_sys::KeyboardEvent| {
                if event.key() == "Escape" {
                    event.prevent_default();
                    on_close.run(());
                    return;
                }
                if event.key() != "Tab" {
                    return;
                }
                let Ok(list) = key_sheet.query_selector_all("a[href], button:not([disabled])")
                else {
                    return;
                };
                let focusable: Vec<web_sys::HtmlElement> = (0..list.length())
                    .filter_map(|i| list.item(i))
                    .filter_map(|n| n.dyn_into::<web_sys::HtmlElement>().ok())
                    .collect();
                let (Some(first), Some(last)) = (focusable.first(), focusable.last()) else {
                    return;
                };
                let active = document().active_element();
                let is = |el: &web_sys::HtmlElement| {
                    active.as_ref().is_some_and(|a| {
                        let el: &web_sys::Element = el.as_ref();
                        a == el
                    })
                };
                if event.shift_key() && is(first) {
                    event.prevent_default();
                    let _ = last.focus();
                } else if !event.shift_key() && is(last) {
                    event.prevent_default();
                    let _ = first.focus();
                }
            },
        );
        let _ = doc.add_event_listener_with_callback("keydown", on_key.as_ref().unchecked_ref());
        on_cleanup_local(move || {
            let doc = document();
            let _ =
                doc.remove_event_listener_with_callback("keydown", on_key.as_ref().unchecked_ref());
            if let Some(body) = &body {
                let _ = body.style().set_property("overflow", &previous_overflow);
            }
            let active = doc.active_element();
            let body_el: Option<web_sys::Element> = doc.body().map(Into::into);
            let restore = match &active {
                None => true,
                Some(active) => {
                    body_el.as_ref() == Some(active) || sheet.contains(Some(active.as_ref()))
                }
            };
            if restore && let Some(previous) = previously_focused {
                let _ = previous.focus();
            }
        });
    });
    let links = MORE_LINKS
        .into_iter()
        .map(|item| nav_link(item, false, path, research_actions, failing_tasks))
        .collect_view();
    view! {
        <div class="mobile-more-backdrop" on:click=move |_| on_close.run(())>
            <div
                node_ref=sheet_ref
                id="mobile-more-menu"
                class="mobile-more-sheet"
                role="dialog"
                aria-modal="true"
                aria-label="More navigation"
                on:click=|event| event.stop_propagation()
            >
                <div class="mobile-more-handle"></div>
                <div class="mobile-more-title">
                    <h2>"More"</h2>
                    <button type="button" class="mobile-more-close" on:click=move |_| on_close.run(())>
                        "Close"
                    </button>
                </div>
                <div class="mobile-more-links">{links}</div>
            </div>
        </div>
    }
}

/// The app navigation; `path` is the normalized current path.
#[component]
pub fn NavBar(#[prop(into)] path: Signal<String>) -> impl IntoView {
    let live = use_live_data();
    let more_open = RwSignal::new(false);
    let workspaces = RwSignal::new(None::<Vec<WorkspaceOverview>>);
    let workspace_load_failed = RwSignal::new(false);
    let refresh = RwSignal::new(0u64);

    Effect::new(move |_| {
        path.track();
        more_open.set(false);
    });

    let on_updated = Closure::<dyn FnMut()>::new(move || refresh.update(|n| *n += 1));
    let win = window();
    let _ = win
        .add_event_listener_with_callback("workspace-updated", on_updated.as_ref().unchecked_ref());
    on_cleanup_local(move || {
        let _ = win.remove_event_listener_with_callback(
            "workspace-updated",
            on_updated.as_ref().unchecked_ref(),
        );
    });
    Effect::new(move |_| {
        refresh.track();
        spawn_scoped(async move {
            match api::fetch_workspaces().await {
                Ok(response) => {
                    workspaces.set(Some(response.workspaces));
                    workspace_load_failed.set(false);
                }
                Err(_) => workspace_load_failed.set(true),
            }
        });
    });

    let research_actions = Signal::derive(move || {
        match workspaces.with(|w| {
            w.as_ref().map(|w| {
                w.iter()
                    .map(|ws| ws.pending_action_count as usize)
                    .sum::<usize>()
            })
        }) {
            Some(total) => Some(total),
            None if workspace_load_failed.get() => None,
            None => Some(0),
        }
    });
    let failing_tasks = Signal::derive(move || {
        Some(live.snapshot.with(|s| {
            s.as_ref().map_or(0, |s| {
                s.tasks
                    .iter()
                    .filter(|t| {
                        t.last_run
                            .as_ref()
                            .is_some_and(|r| r.status == RunStatus::Error)
                    })
                    .count()
            })
        }))
    });
    let more_active = move || {
        MORE_LINKS
            .iter()
            .any(|item| is_path_active(&path.get(), item))
    };
    let close = Callback::new(move |()| more_open.set(false));

    let primary_sidebar = PRIMARY_LINKS
        .into_iter()
        .map(|item| nav_link(item, false, path, research_actions, failing_tasks))
        .collect_view();
    let more_sidebar = MORE_LINKS
        .into_iter()
        .map(|item| nav_link(item, false, path, research_actions, failing_tasks))
        .collect_view();
    let primary_mobile = PRIMARY_LINKS
        .into_iter()
        .map(|item| nav_link(item, true, path, research_actions, failing_tasks))
        .collect_view();

    view! {
        <aside class="app-sidebar">
            <div class="sidebar-header">
                <Brand/>
            </div>
            <nav class="sidebar-nav" aria-label="Primary navigation">
                <div class="sidebar-group">{primary_sidebar}</div>
                <div class="sidebar-group">
                    <div class="sidebar-group-label">"More"</div>
                    {more_sidebar}
                </div>
            </nav>
            <div class="sidebar-footer">
                <ConnectionBadge/>
            </div>
        </aside>

        <header class="mobile-header">
            <Brand/>
            <ConnectionBadge/>
        </header>

        <nav class="mobile-bottom-nav" aria-label="Primary navigation">
            {primary_mobile}
            <button
                type="button"
                class=move || {
                    format!(
                        "mobile-nav-link {}",
                        if more_open.get() || more_active() { "active" } else { "" },
                    )
                }
                on:click=move |_| more_open.update(|open| *open = !*open)
                aria-expanded=move || more_open.get().to_string()
                aria-controls="mobile-more-menu"
            >
                <NavIcon icon=Icon::More/>
                <span>"More"</span>
                <ActionBadge value=failing_tasks/>
            </button>
        </nav>

        <Show when=move || more_open.get()>
            <MoreSheet path research_actions failing_tasks on_close=close/>
        </Show>
    }
}
