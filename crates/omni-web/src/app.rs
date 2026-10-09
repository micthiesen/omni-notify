//! Router and shell (`App.tsx`, `main.tsx`).

use leptos::prelude::*;
use omni_web_kit::components::{NavBar, SectionNav};
use omni_web_kit::live::provide_live_data;
use omni_web_kit::router::{Link, provide_router};
use omni_web_pages::{
    BriefingsPage, ClaudePage, FeedbackPage, McpPage, MediaDetailPage, MediaPage, PetsPage,
    PodcastDetailPage, PodcastsPage, PodsDetailPage, PodsPage, RemindersPage, WorkspacesPage,
};
use wasm_bindgen::JsCast as _;

use crate::pages::{
    CostsPage, DataPage, EmailActivityPage, HomePage, LivestreamIntelligencePage, OperationsPage,
    StreamerPage,
};
use crate::routes::{Route, match_route, normalize_path, page_title, workspace_ids};

#[component]
fn NotFound() -> impl IntoView {
    view! {
        <div class="not-found-page">
            <span class="home-eyebrow">"404"</span>
            <h1>"Page Not Found"</h1>
            <p class="page-subtitle">"This link does not lead to a page in Omni Notify."</p>
            <Link to="/" class="section-view-all">"Back to Home ›"</Link>
        </div>
    }
}

fn render(route: Route, path: Memo<String>) -> AnyView {
    match route {
        Route::Home => view! { <HomePage/> }.into_any(),
        Route::Media => view! { <MediaPage/> }.into_any(),
        Route::MediaDetail(id) => view! { <MediaDetailPage id/> }.into_any(),
        Route::Podcasts => view! { <PodcastsPage/> }.into_any(),
        Route::PodcastDetail(id) => view! { <PodcastDetailPage id/> }.into_any(),
        Route::Feedback(kind, id) => view! { <FeedbackPage kind id/> }.into_any(),
        Route::Pods => view! { <PodsPage/> }.into_any(),
        Route::PodsDetail(id) => view! { <PodsDetailPage id/> }.into_any(),
        Route::Streamer(streamer_id) => view! { <StreamerPage streamer_id/> }.into_any(),
        Route::StreamerIntelligence(streamer_id) => {
            view! { <LivestreamIntelligencePage streamer_id/> }.into_any()
        }
        Route::Workspaces => {
            let ids = Memo::new(move |_| path.with(|p| workspace_ids(p)));
            let workspace_id = Signal::derive(move || ids.get().0);
            let subject_id = Signal::derive(move || ids.get().1);
            view! { <WorkspacesPage workspace_id subject_id/> }.into_any()
        }
        Route::Briefings => view! { <BriefingsPage/> }.into_any(),
        Route::Emails => view! { <EmailActivityPage/> }.into_any(),
        Route::Data => view! { <DataPage/> }.into_any(),
        Route::Costs => view! { <CostsPage/> }.into_any(),
        Route::Operations => view! { <OperationsPage/> }.into_any(),
        Route::Reminders => view! { <RemindersPage/> }.into_any(),
        Route::Pets => view! { <PetsPage/> }.into_any(),
        Route::Mcp => view! { <McpPage/> }.into_any(),
        Route::Claude => view! { <ClaudePage/> }.into_any(),
        Route::NotFound => view! { <NotFound/> }.into_any(),
    }
}

#[component]
pub fn App() -> impl IntoView {
    let raw_path = provide_router();
    provide_live_data();
    let path = Memo::new(move |_| normalize_path(&raw_path.get()));
    let route = Memo::new(move |_| path.with(|p| match_route(p)));

    // Move focus to the page on navigation (not on first load).
    Effect::new(move |previous: Option<String>| {
        let current = path.get();
        if previous.as_ref().is_some_and(|p| *p != current)
            && let Some(main) = document()
                .get_element_by_id("main-content")
                .and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok())
        {
            let options = web_sys::FocusOptions::new();
            options.set_prevent_scroll(true);
            let _ = main.focus_with_options(&options);
        }
        current
    });
    Effect::new(move |_| document().set_title(&path.with(|p| page_title(p))));

    let nav_path = Signal::derive(move || path.get());
    view! {
        <div class="app-shell">
            <a href="#main-content" class="skip-link">"Skip to Content"</a>
            <NavBar path=nav_path/>
            <main
                id="main-content"
                tabindex="-1"
                class=move || format!("page {}", if path.get() == "/data" { "page-data" } else { "" })
            >
                <SectionNav path=nav_path/>
                {move || render(route.get(), path)}
            </main>
        </div>
    }
}
