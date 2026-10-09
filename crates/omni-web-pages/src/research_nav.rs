//! The phone-only `Workspaces | Briefings` switch under the Research titles
//! (the phone tab bar has a single Research tab).

use leptos::prelude::*;
use omni_web_kit::router::Link;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ResearchTab {
    Workspaces,
    Briefings,
}

#[component]
pub fn ResearchSwitch(current: ResearchTab) -> impl IntoView {
    let item = move |tab: ResearchTab, to: &'static str, label: &'static str| {
        let here = (tab == current).then(|| "page".to_owned());
        view! {
            <Link to=to class="seg-btn" aria_current=here>
                {label}
            </Link>
        }
    };
    view! {
        <nav class="seg research-switch only-phone" aria-label="Research">
            {item(ResearchTab::Workspaces, "/workspaces", "Workspaces")}
            {item(ResearchTab::Briefings, "/briefings", "Briefings")}
        </nav>
    }
}
