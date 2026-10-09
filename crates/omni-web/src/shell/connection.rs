//! The connection readout (rail footer and phone top bar), mapped 1:1 onto
//! `ConnectionState`. The dot pops once per snapshot.

use leptos::html::Span;
use leptos::prelude::*;
use omni_web_kit::hooks::use_now;
use omni_web_kit::live::{ConnectionState, use_live_data};

/// `(class, text, detail)` for a state.
pub fn connection_copy(
    state: ConnectionState,
    error: bool,
    seconds: Option<u64>,
) -> (&'static str, String, Option<String>) {
    match state {
        ConnectionState::Live => (
            "",
            "Live".to_owned(),
            Some(match seconds {
                Some(s) if s < 60 => format!("{s}s"),
                Some(s) => format!("{}m", s / 60),
                None => "now".to_owned(),
            }),
        ),
        ConnectionState::Polling if error => {
            ("fault", "Offline".to_owned(), Some("retry".to_owned()))
        }
        ConnectionState::Polling => ("warn", "Polling".to_owned(), Some("every 10 s".to_owned())),
        ConnectionState::Connecting if error => {
            ("fault", "Offline".to_owned(), Some("retry".to_owned()))
        }
        ConnectionState::Connecting => ("warn", "Reconnecting…".to_owned(), None),
    }
}

/// `compact` renders only the dot (phone top bar). Clicking reloads unless
/// live.
#[component]
pub fn Connection(#[prop(optional)] compact: bool) -> impl IntoView {
    let live = use_live_data();
    let now = use_now(1000);
    let dot = NodeRef::<Span>::new();
    let copy = Memo::new(move |_| {
        let updated = live.updated_at.get();
        let seconds =
            (updated > 0.0).then(|| ((now.get() - updated) / 1000.0).max(0.0).floor() as u64);
        connection_copy(
            live.connection.get(),
            live.error.with(Option::is_some),
            seconds,
        )
    });
    // Pop the dot on every snapshot while live.
    Effect::new(move |previous: Option<f64>| {
        let updated = live.updated_at.get();
        if previous.is_some_and(|p| p != updated)
            && live.connection.get_untracked() == ConnectionState::Live
            && let Some(el) = dot.get_untracked()
        {
            let list = el.class_list();
            let _ = list.remove_1("pop");
            let _ = el.offset_width();
            let _ = list.add_1("pop");
        }
        updated
    });
    let dot_class = move || {
        let (tone, _, _) = copy.get();
        let pulse = live.connection.get() == ConnectionState::Connecting && tone == "warn";
        format!("conn-dot {tone}{}", if pulse { " pulse" } else { "" })
    };
    let label = move || {
        let (_, text, detail) = copy.get();
        match detail {
            Some(d) => format!("{text} · {d}"),
            None => text,
        }
    };
    view! {
        <button
            type="button"
            class=move || format!("conn {}", copy.get().0)
            title=move || {
                if live.connection.get() == ConnectionState::Live {
                    "Connected: snapshots stream live".to_owned()
                } else {
                    "Reload".to_owned()
                }
            }
            aria-label=move || format!("Connection: {}", label())
            on:click=move |_| {
                if live.connection.get_untracked() != ConnectionState::Live {
                    let _ = window().location().reload();
                }
            }
        >
            <span node_ref=dot class=dot_class aria-hidden="true"></span>
            {(!compact).then(|| view! {
                <span role="status">{move || copy.get().1}</span>
                <span class="detail num">{move || copy.get().2}</span>
            })}
        </button>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_maps_each_state() {
        assert_eq!(
            connection_copy(ConnectionState::Live, false, Some(2)),
            ("", "Live".into(), Some("2s".into()))
        );
        assert_eq!(
            connection_copy(ConnectionState::Polling, false, None).0,
            "warn"
        );
        assert_eq!(
            connection_copy(ConnectionState::Polling, true, None).1,
            "Offline"
        );
        assert_eq!(
            connection_copy(ConnectionState::Connecting, false, None).1,
            "Reconnecting…"
        );
    }
}
