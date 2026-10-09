//! Live channel, running, failing and next-run tiles.

use leptos::prelude::*;
use omni_api::runs::RunStatus;

use crate::api::Snapshot;
use crate::hooks::use_now;
use crate::utils::format::{format_countdown, task_label};
use crate::utils::js::parse_date_ms;

struct Tile {
    label: &'static str,
    value: String,
    detail: Option<String>,
    tone: &'static str,
}

fn tiles(snapshot: &Snapshot, now: f64) -> Vec<Tile> {
    let live_count = snapshot.streamers.iter().filter(|s| s.is_live()).count();
    let running = snapshot.tasks.iter().filter(|t| t.running).count();
    let failing = snapshot
        .tasks
        .iter()
        .filter(|t| {
            t.last_run
                .as_ref()
                .is_some_and(|r| r.status == RunStatus::Error)
        })
        .count();
    let next = snapshot
        .tasks
        .iter()
        .filter_map(|t| Some((t, parse_date_ms(t.next_runs.first()?)?)))
        .min_by(|a, b| a.1.total_cmp(&b.1));

    let mut tiles = Vec::new();
    if !snapshot.streamers.is_empty() {
        tiles.push(Tile {
            label: "Live Channels",
            value: live_count.to_string(),
            detail: None,
            tone: if live_count > 0 { "live" } else { "" },
        });
    }
    tiles.push(Tile {
        label: "Tasks Running",
        value: running.to_string(),
        detail: Some(format!("{} registered", snapshot.tasks.len())),
        tone: if running > 0 { "accent" } else { "" },
    });
    tiles.push(Tile {
        label: "Tasks Failing",
        value: failing.to_string(),
        detail: None,
        tone: if failing > 0 { "danger" } else { "" },
    });
    tiles.push(match next {
        Some((task, at)) => Tile {
            label: "Next Run",
            value: format_countdown(at - now),
            detail: Some(task_label(&task.name, task.display_name.as_deref())),
            tone: "",
        },
        None => Tile {
            label: "Next Run",
            value: "—".to_owned(),
            detail: None,
            tone: "",
        },
    });
    tiles
}

#[component]
pub fn StatStrip(#[prop(into)] snapshot: Signal<Snapshot>) -> impl IntoView {
    let now = use_now(1000);
    view! {
        <div class="stat-strip">
            {move || {
                let now = now.get();
                snapshot
                    .with(|s| tiles(s, now))
                    .into_iter()
                    .map(|tile| {
                        view! {
                            <div class=format!("stat-tile {}", tile.tone)>
                                <span class="stat-label">{tile.label}</span>
                                <span class="stat-value">{tile.value}</span>
                                {tile.detail.map(|d| view! { <span class="stat-detail">{d}</span> })}
                            </div>
                        }
                    })
                    .collect_view()
            }}
        </div>
    }
}
