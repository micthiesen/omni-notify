//! Sub-navigation for the Listen and Research sections
//! (`components/SectionNav.tsx`).

use leptos::prelude::*;

use crate::router::Link;

fn active(path: &str, to: &str) -> bool {
    path == to || path.starts_with(&format!("{to}/"))
}

fn section(path: &str) -> Option<(&'static str, [(&'static str, &'static str); 2])> {
    if path.starts_with("/podcasts") || path.starts_with("/pods") {
        Some((
            "Listen",
            [("/podcasts", "Podcast Picks"), ("/pods", "PressPods")],
        ))
    } else if path.starts_with("/workspaces") || path.starts_with("/briefings") {
        Some((
            "Research",
            [("/workspaces", "Workspaces"), ("/briefings", "Briefings")],
        ))
    } else {
        None
    }
}

#[component]
pub fn SectionNav(#[prop(into)] path: Signal<String>) -> impl IntoView {
    let current = Memo::new(move |_| path.with(|p| section(p).map(|(label, _)| label)));
    move || {
        current.get().and_then(|_| {
            let p = path.get_untracked();
            let (label, links) = section(&p)?;
            let links = links
                .into_iter()
                .map(|(to, text)| {
                    let class = move || {
                        format!("section-nav-link {}", if active(&path.get(), to) { "active" } else { "" })
                    };
                    let current = move || active(&path.get(), to).then(|| "page".to_owned());
                    view! {
                        <Link to=to class=Signal::derive(class) aria_current=Signal::derive(current)>
                            {text}
                        </Link>
                    }
                })
                .collect_view();
            Some(view! {
                <nav class="section-nav" aria-label=format!("{label} sections")>
                    <span class="section-nav-label">{label}</span>
                    {links}
                </nav>
            })
        })
    }
}
