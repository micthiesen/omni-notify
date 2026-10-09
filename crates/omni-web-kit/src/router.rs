//! History-based client routing (`frontend/src/router.tsx`).

use leptos::prelude::*;
use wasm_bindgen::JsCast as _;
use wasm_bindgen::closure::Closure;

use crate::task::on_cleanup_local;

/// Paths served by the server itself rather than the SPA (`/reminders` is a
/// server-rendered page with its own strict CSP); links to them are full loads.
pub const SERVER_PAGES: [&str; 1] = ["/reminders"];

pub fn is_server_page(to: &str) -> bool {
    let path = to.split(['?', '#']).next().unwrap_or(to);
    SERVER_PAGES.contains(&path.trim_end_matches('/'))
}

/// Push `to` onto the history stack, notify `use_path`, and scroll to top.
pub fn navigate(to: &str) {
    if is_server_page(to) {
        let _ = window().location().assign(to);
        return;
    }
    let win = window();
    if let Ok(history) = win.history() {
        let _ = history.push_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(to));
    }
    if let Ok(event) = web_sys::PopStateEvent::new("popstate") {
        let _ = win.dispatch_event(&event);
    }
    let options = web_sys::ScrollToOptions::new();
    options.set_top(0.0);
    options.set_left(0.0);
    options.set_behavior(web_sys::ScrollBehavior::Instant);
    win.scroll_to_with_scroll_to_options(&options);
}

/// The current `location.pathname`, kept in sync with `popstate`.
#[derive(Clone, Copy)]
pub struct RouterContext {
    pub path: ReadSignal<String>,
}

fn current_pathname() -> String {
    window()
        .location()
        .pathname()
        .unwrap_or_else(|_| "/".to_owned())
}

/// Creates the path signal and provides it as context (`usePath`).
pub fn provide_router() -> ReadSignal<String> {
    let (path, set_path) = signal(current_pathname());
    let on_pop = Closure::<dyn FnMut()>::new(move || set_path.set(current_pathname()));
    let win = window();
    let _ = win.add_event_listener_with_callback("popstate", on_pop.as_ref().unchecked_ref());
    on_cleanup_local(move || {
        let _ =
            win.remove_event_listener_with_callback("popstate", on_pop.as_ref().unchecked_ref());
    });
    provide_context(RouterContext { path });
    path
}

/// The raw current path (`usePath`).
pub fn use_path() -> ReadSignal<String> {
    match use_context::<RouterContext>() {
        Some(ctx) => ctx.path,
        None => signal(current_pathname()).0,
    }
}

/// An anchor that navigates client-side on an unmodified primary click.
#[component]
pub fn Link(
    #[prop(into)] to: String,
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(into, optional)] title: MaybeProp<String>,
    #[prop(into, optional)] aria_label: MaybeProp<String>,
    #[prop(into, optional)] aria_current: MaybeProp<String>,
    children: Children,
) -> impl IntoView {
    let target = to.clone();
    let on_click = move |event: web_sys::MouseEvent| {
        if event.default_prevented()
            || event.button() != 0
            || event.meta_key()
            || event.ctrl_key()
            || event.shift_key()
            || event.alt_key()
        {
            return;
        }
        if is_server_page(&target) {
            return;
        }
        event.prevent_default();
        navigate(&target);
    };
    view! {
        <a
            href=to
            class=move || class.get()
            title=move || title.get()
            aria-label=move || aria_label.get()
            aria-current=move || aria_current.get()
            on:click=on_click
        >
            {children()}
        </a>
    }
}

#[cfg(test)]
mod tests {
    use super::is_server_page;

    #[test]
    fn reminders_is_a_full_page_load() {
        assert!(is_server_page("/reminders"));
        assert!(is_server_page("/reminders/"));
        assert!(is_server_page("/reminders?x=1"));
        assert!(!is_server_page("/media"));
    }
}
