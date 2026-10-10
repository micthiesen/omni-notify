//! Stroke icons (24 px grid, 1.6 stroke, `currentColor`).

use leptos::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Icon {
    Home,
    Live,
    Film,
    Headphones,
    Mic,
    Doc,
    Mail,
    CheckSquare,
    Paw,
    Pulse,
    Coin,
    Plug,
    Terminal,
    Database,
    Search,
    Grid,
    ChevronRight,
    ChevronDown,
    ChevronUp,
    Close,
    External,
    Download,
    Play,
    Pause,
    Refresh,
    Sidebar,
    Star,
    Copy,
    Check,
    Plus,
    Trash,
    Logs,
    Inbox,
    Clock,
    Alert,
    Package,
    Calendar,
    Pencil,
}

/// Size modifier for [`Glyph`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IconSize {
    Small,
    #[default]
    Medium,
    Large,
}

fn shapes(icon: Icon) -> AnyView {
    match icon {
        Icon::Home => view! {
            <path d="m3 11 9-8 9 8"></path>
            <path d="M5 10v10h14V10"></path>
        }
        .into_any(),
        Icon::Live => view! {
            <circle cx="12" cy="12" r="2"></circle>
            <path d="M8.5 8.5a5 5 0 0 0 0 7M15.5 8.5a5 5 0 0 1 0 7M5.6 5.6a9 9 0 0 0 0 12.8M18.4 5.6a9 9 0 0 1 0 12.8"></path>
        }
        .into_any(),
        Icon::Film => view! {
            <rect x="3" y="4" width="18" height="16" rx="2"></rect>
            <path d="M7 4v16M17 4v16M3 9h4M3 15h4M17 9h4M17 15h4"></path>
        }
        .into_any(),
        Icon::Headphones => view! {
            <path d="M4 14a8 8 0 0 1 16 0"></path>
            <path d="M4 14v4a2 2 0 0 0 2 2h2v-6H4M20 14v4a2 2 0 0 1-2 2h-2v-6h4"></path>
        }
        .into_any(),
        Icon::Mic => view! {
            <rect x="9" y="3" width="6" height="11" rx="3"></rect>
            <path d="M5 11a7 7 0 0 0 14 0M12 18v3"></path>
        }
        .into_any(),
        Icon::Doc => view! {
            <path d="M6 3h9l4 4v14H6z"></path>
            <path d="M9 11h7M9 15h7M14 3v5h5"></path>
        }
        .into_any(),
        Icon::Mail => view! {
            <rect x="3" y="5" width="18" height="14" rx="2"></rect>
            <path d="m4 7 8 6 8-6"></path>
        }
        .into_any(),
        Icon::CheckSquare => view! {
            <rect x="4" y="4" width="16" height="16" rx="3"></rect>
            <path d="m8.5 12 2.5 2.5 4.5-5"></path>
        }
        .into_any(),
        Icon::Paw => view! {
            <circle cx="7.5" cy="9" r="1.6"></circle>
            <circle cx="12" cy="6.5" r="1.6"></circle>
            <circle cx="16.5" cy="9" r="1.6"></circle>
            <path d="M8 17c0-3 1.8-5.5 4-5.5s4 2.5 4 5.5c0 1.7-1.6 2.5-4 2.5s-4-.8-4-2.5Z"></path>
        }
        .into_any(),
        Icon::Pulse => view! { <path d="M3 12h4l2.5-6 5 12 2.5-6h4"></path> }.into_any(),
        Icon::Coin => view! {
            <circle cx="12" cy="12" r="9"></circle>
            <path d="M14.8 9c-.6-.8-1.6-1.3-2.8-1.3-1.6 0-2.8.9-2.8 2.1 0 3.3 5.8 1.3 5.8 4.6 0 1.3-1.3 2.3-3 2.3-1.3 0-2.4-.5-3.1-1.4M12 6v12"></path>
        }
        .into_any(),
        Icon::Plug => view! {
            <path d="M9 3v5M15 3v5M6 8h12v3a6 6 0 0 1-12 0zM12 17v4"></path>
        }
        .into_any(),
        Icon::Terminal => view! {
            <rect x="3" y="4" width="18" height="16" rx="2"></rect>
            <path d="m7 9 3 3-3 3M13 15h4"></path>
        }
        .into_any(),
        Icon::Database => view! {
            <ellipse cx="12" cy="5.5" rx="7.5" ry="2.5"></ellipse>
            <path d="M4.5 5.5v13c0 1.4 3.4 2.5 7.5 2.5s7.5-1.1 7.5-2.5v-13M4.5 12c0 1.4 3.4 2.5 7.5 2.5s7.5-1.1 7.5-2.5"></path>
        }
        .into_any(),
        Icon::Search => view! {
            <circle cx="11" cy="11" r="6.5"></circle>
            <path d="m16 16 4.5 4.5"></path>
        }
        .into_any(),
        Icon::Grid => view! {
            <rect x="4" y="4" width="6.5" height="6.5" rx="1.5"></rect>
            <rect x="13.5" y="4" width="6.5" height="6.5" rx="1.5"></rect>
            <rect x="4" y="13.5" width="6.5" height="6.5" rx="1.5"></rect>
            <rect x="13.5" y="13.5" width="6.5" height="6.5" rx="1.5"></rect>
        }
        .into_any(),
        Icon::ChevronRight => view! { <path d="m9 6 6 6-6 6"></path> }.into_any(),
        Icon::ChevronDown => view! { <path d="m6 9 6 6 6-6"></path> }.into_any(),
        Icon::ChevronUp => view! { <path d="m6 15 6-6 6 6"></path> }.into_any(),
        Icon::Close => view! { <path d="M6 6l12 12M18 6 6 18"></path> }.into_any(),
        Icon::External => view! { <path d="M14 4h6v6M20 4l-9 9M18 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h5"></path> }
            .into_any(),
        Icon::Download => view! { <path d="M12 4v11M7 10l5 5 5-5M5 20h14"></path> }.into_any(),
        Icon::Play => view! { <path d="M8 5.5v13l11-6.5z"></path> }.into_any(),
        Icon::Pause => view! { <path d="M8 5h3v14H8zM13 5h3v14h-3z"></path> }.into_any(),
        Icon::Refresh => view! { <path d="M20 12a8 8 0 1 1-2.3-5.7M20 4v5h-5"></path> }.into_any(),
        Icon::Sidebar => view! {
            <rect x="3" y="4" width="18" height="16" rx="2"></rect>
            <path d="M9 4v16"></path>
        }
        .into_any(),
        Icon::Star => view! { <path d="m12 4 2.4 5 5.4.7-4 3.7 1 5.4L12 16.2 7.2 18.8l1-5.4-4-3.7 5.4-.7z"></path> }
            .into_any(),
        Icon::Copy => view! {
            <rect x="8" y="8" width="12" height="12" rx="2"></rect>
            <path d="M16 8V5a1 1 0 0 0-1-1H5a1 1 0 0 0-1 1v10a1 1 0 0 0 1 1h3"></path>
        }
        .into_any(),
        Icon::Check => view! { <path d="m5 12.5 4.5 4.5L19 7.5"></path> }.into_any(),
        Icon::Plus => view! { <path d="M12 5v14M5 12h14"></path> }.into_any(),
        Icon::Trash => view! { <path d="M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13"></path> }.into_any(),
        Icon::Logs => view! { <path d="M4 6h16M4 10h10M4 14h16M4 18h8"></path> }.into_any(),
        Icon::Inbox => view! {
            <path d="M4 13h4l1.5 3h5L16 13h4"></path>
            <path d="M6 5h12l2 8v6H4v-6z"></path>
        }
        .into_any(),
        Icon::Clock => view! {
            <circle cx="12" cy="12" r="8.5"></circle>
            <path d="M12 7.5V12l3 2"></path>
        }
        .into_any(),
        Icon::Alert => view! { <path d="M12 4 2.8 19.5h18.4zM12 10v4.5M12 17v.5"></path> }.into_any(),
        Icon::Package => view! {
            <path d="M4 7.5 12 3.5l8 4v9l-8 4-8-4z"></path>
            <path d="m4 7.5 8 4 8-4M12 11.5v9M8 5.5l8 4"></path>
        }
        .into_any(),
        Icon::Calendar => view! {
            <rect x="4" y="5.5" width="16" height="14.5" rx="2"></rect>
            <path d="M4 10h16M8.5 3.5v4M15.5 3.5v4"></path>
        }
        .into_any(),
        Icon::Pencil => view! { <path d="M4 20h4L19 9l-4-4L4 16zM13.5 6.5l4 4"></path> }.into_any(),
    }
}

/// An inline icon; decorative unless `label` is set.
#[component]
pub fn Glyph(
    icon: Icon,
    #[prop(optional)] size: IconSize,
    #[prop(into, optional)] label: Option<String>,
    #[prop(into, optional)] class: Option<&'static str>,
) -> impl IntoView {
    let size = match size {
        IconSize::Small => " sm",
        IconSize::Medium => "",
        IconSize::Large => " lg",
    };
    let hidden = label.is_none().then_some("true");
    let role = label.is_some().then_some("img");
    view! {
        <svg
            class=format!("icon{size} {}", class.unwrap_or(""))
            viewBox="0 0 24 24"
            aria-hidden=hidden
            role=role
            aria-label=label
        >
            {shapes(icon)}
        </svg>
    }
}
