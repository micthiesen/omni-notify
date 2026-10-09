//! Shared view hooks.

use std::rc::Rc;
use std::time::Duration;

use leptos::html::Div;
use leptos::prelude::*;
use wasm_bindgen::JsCast as _;
use wasm_bindgen::closure::Closure;

use crate::task::{on_cleanup_local, sleep, spawn_scoped};
use crate::utils::js::now_ms;

/// Re-renders on an interval; the current epoch ms (`useNow`).
pub fn use_now(interval_ms: u64) -> ReadSignal<f64> {
    let (now, set_now) = signal(now_ms());
    spawn_scoped(async move {
        loop {
            sleep(Duration::from_millis(interval_ms)).await;
            set_now.set(now_ms());
        }
    });
    now
}

fn page_visible() -> bool {
    document().visibility_state() == web_sys::VisibilityState::Visible
}

/// `document.visibilityState === "visible"`, kept current.
pub fn use_page_visible() -> ReadSignal<bool> {
    let (visible, set_visible) = signal(page_visible());
    let handler = Closure::<dyn FnMut()>::new(move || set_visible.set(page_visible()));
    let doc = document();
    let _ =
        doc.add_event_listener_with_callback("visibilitychange", handler.as_ref().unchecked_ref());
    on_cleanup_local(move || {
        let _ = doc.remove_event_listener_with_callback(
            "visibilitychange",
            handler.as_ref().unchecked_ref(),
        );
    });
    visible
}

/// Run `load` now and every `interval` while the tab is visible
/// (`useVisiblePoll`). Hidden tabs stop polling; returning to the tab fetches
/// immediately. A change of `key` restarts the loop (use it for filters). The
/// returned callback forces an immediate reload.
pub fn use_visible_poll<T, E, Fut, L, S, F>(
    key: Signal<String>,
    load: L,
    on_success: S,
    on_failure: F,
    interval: Duration,
) -> Callback<()>
where
    T: 'static,
    E: 'static,
    Fut: std::future::Future<Output = Result<T, E>> + 'static,
    L: Fn() -> Fut + 'static,
    S: Fn(T) + 'static,
    F: Fn(E) + 'static,
{
    let visible = use_page_visible();
    let nonce = RwSignal::new(0u64);
    let load = Rc::new(load);
    let on_success = Rc::new(on_success);
    let on_failure = Rc::new(on_failure);
    Effect::new(move |_| {
        let is_visible = visible.get();
        key.track();
        nonce.track();
        if !is_visible {
            return;
        }
        let (load, on_success, on_failure) = (load.clone(), on_success.clone(), on_failure.clone());
        spawn_scoped(async move {
            loop {
                match load().await {
                    Ok(value) => on_success(value),
                    Err(error) => on_failure(error),
                }
                sleep(interval).await;
            }
        });
    });
    Callback::new(move |()| nonce.update(|n| *n += 1))
}

const FOCUSABLE: &str = "a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex=\"-1\"])";

fn focusable_in(panel: &web_sys::Element) -> Vec<web_sys::HtmlElement> {
    let Ok(list) = panel.query_selector_all(FOCUSABLE) else {
        return Vec::new();
    };
    (0..list.length())
        .filter_map(|i| list.item(i))
        .filter_map(|node| node.dyn_into::<web_sys::HtmlElement>().ok())
        .filter(|el| el.get_client_rects().length() > 0)
        .collect()
}

fn active_element() -> Option<web_sys::Element> {
    document().active_element()
}

fn same_element(a: Option<&web_sys::Element>, b: &web_sys::HtmlElement) -> bool {
    a.is_some_and(|a| {
        let b: &web_sys::Element = b.as_ref();
        a == b
    })
}

/// Shared keyboard, focus-trap and scroll-lock behavior for modal panels
/// (`useModal`). Escape calls `on_close`.
pub fn use_modal(on_close: impl Fn() + 'static) -> NodeRef<Div> {
    let panel_ref = NodeRef::<Div>::new();
    let on_close: Rc<dyn Fn()> = Rc::new(on_close);
    let installed = StoredValue::new(false);
    Effect::new(move |_| {
        let Some(panel) = panel_ref.get() else { return };
        if installed.get_value() {
            return;
        }
        installed.set_value(true);
        let panel: web_sys::HtmlElement = panel.into();
        let previous_focus =
            active_element().and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok());
        let body = document().body();
        let previous_overflow = body
            .as_ref()
            .and_then(|b| b.style().get_property_value("overflow").ok())
            .unwrap_or_default();
        if let Some(body) = &body {
            let _ = body.style().set_property("overflow", "hidden");
        }
        let panel_el: web_sys::Element = panel.clone().into();
        let close_button = panel_el
            .query_selector(".inspector-close, .palette-close")
            .ok()
            .flatten()
            .and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok());
        let first = close_button.or_else(|| focusable_in(&panel_el).into_iter().next());
        let _ = first.unwrap_or_else(|| panel.clone()).focus();

        let on_close = on_close.clone();
        let key_panel = panel.clone();
        let key_panel_el = panel_el.clone();
        let on_key = Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(
            move |event: web_sys::KeyboardEvent| {
                if event.key() == "Escape" {
                    event.prevent_default();
                    event.stop_propagation();
                    on_close();
                }
                if event.key() != "Tab" {
                    return;
                }
                let elements = focusable_in(&key_panel_el);
                let (Some(first), Some(last)) = (elements.first(), elements.last()) else {
                    event.prevent_default();
                    let _ = key_panel.focus();
                    return;
                };
                let active = active_element();
                let on_panel = same_element(active.as_ref(), &key_panel);
                if event.shift_key() && (same_element(active.as_ref(), first) || on_panel) {
                    event.prevent_default();
                    let _ = last.focus();
                } else if !event.shift_key() && (same_element(active.as_ref(), last) || on_panel) {
                    event.prevent_default();
                    let _ = first.focus();
                }
            },
        );
        let focus_panel = panel.clone();
        let focus_panel_el = panel_el.clone();
        let on_focus =
            Closure::<dyn FnMut(web_sys::FocusEvent)>::new(move |event: web_sys::FocusEvent| {
                let target = event
                    .target()
                    .and_then(|t| t.dyn_into::<web_sys::Node>().ok());
                if let Some(target) = target
                    && !focus_panel_el.contains(Some(&target))
                {
                    let next = focusable_in(&focus_panel_el)
                        .into_iter()
                        .next()
                        .unwrap_or_else(|| focus_panel.clone());
                    let _ = next.focus();
                }
            });
        let doc = document();
        let _ = doc.add_event_listener_with_callback("keydown", on_key.as_ref().unchecked_ref());
        let _ = doc.add_event_listener_with_callback("focusin", on_focus.as_ref().unchecked_ref());
        on_cleanup_local(move || {
            let _ =
                doc.remove_event_listener_with_callback("keydown", on_key.as_ref().unchecked_ref());
            let _ = doc
                .remove_event_listener_with_callback("focusin", on_focus.as_ref().unchecked_ref());
            if let Some(body) = &body {
                let _ = body.style().set_property("overflow", &previous_overflow);
            }
            if let Some(previous) = previous_focus
                && previous.is_connected()
            {
                let _ = previous.focus();
            }
        });
    });
    panel_ref
}

/// `?recommendation=` from Pushover deep links: scrolls the matching
/// `#recommendation-<id>` card into view once `loaded` (`useRecHighlight`).
pub fn use_rec_highlight(loaded: Signal<bool>) -> Option<String> {
    let highlighted = query_param("recommendation");
    let target = highlighted.clone();
    Effect::new(move |_| {
        let Some(id) = target.as_ref() else { return };
        if !loaded.get() {
            return;
        }
        scroll_into_view_center(&format!("recommendation-{id}"));
    });
    highlighted
}

/// The current URL's query parameter `name`.
pub fn query_param(name: &str) -> Option<String> {
    let search = window().location().search().ok()?;
    web_sys::UrlSearchParams::new_with_str(&search)
        .ok()?
        .get(name)
}

/// Smoothly scrolls `#id` to the vertical center (`scrollIntoView`).
pub fn scroll_into_view_center(id: &str) {
    if let Some(element) = document().get_element_by_id(id) {
        let options = web_sys::ScrollIntoViewOptions::new();
        options.set_behavior(web_sys::ScrollBehavior::Smooth);
        options.set_block(web_sys::ScrollLogicalPosition::Center);
        element.scroll_into_view_with_scroll_into_view_options(&options);
    }
}

/// Deep-link target for `?section=&target=` style links: the `(section,
/// target)` query parameters, read once on mount.
pub fn use_deep_link_target() -> (Option<String>, Option<String>) {
    (query_param("section"), query_param("target"))
}

/// `matchMedia(query).matches`, kept current.
pub fn use_media_query(query: &'static str) -> ReadSignal<bool> {
    let list = window().match_media(query).ok().flatten();
    let (matches, set_matches) =
        signal(list.as_ref().is_some_and(web_sys::MediaQueryList::matches));
    if let Some(list) = list {
        let watched = list.clone();
        let handler = Closure::<dyn FnMut()>::new(move || set_matches.set(watched.matches()));
        let _ = list.add_event_listener_with_callback("change", handler.as_ref().unchecked_ref());
        on_cleanup_local(move || {
            let _ = list
                .remove_event_listener_with_callback("change", handler.as_ref().unchecked_ref());
        });
    }
    matches
}

/// The wide tier (>= 1280 px): two-column layouts and docked inspectors.
pub fn use_is_wide() -> ReadSignal<bool> {
    use_media_query("(min-width: 1280px)")
}

/// The phone tier (<= 899 px).
pub fn use_is_phone() -> ReadSignal<bool> {
    use_media_query("(max-width: 899px)")
}

/// Replaces `?name=` in the URL without navigating (`None` removes it).
pub fn replace_query_param(name: &str, value: Option<&str>) {
    let location = window().location();
    let Ok(href) = location.href() else { return };
    let Ok(url) = web_sys::Url::new(&href) else {
        return;
    };
    let params = url.search_params();
    match value {
        Some(value) => params.set(name, value),
        None => params.delete(name),
    }
    replace_url(&url);
}

/// `#key=value` of the current URL.
pub fn hash_param(key: &str) -> Option<String> {
    let hash = window().location().hash().ok()?;
    let rest = hash.strip_prefix('#')?;
    let value = rest.strip_prefix(key)?.strip_prefix('=')?;
    crate::utils::js::decode_component(value)
}

/// Sets or clears `#key=value` without navigating.
pub fn replace_hash_param(key: &str, value: Option<&str>) {
    let Ok(href) = window().location().href() else {
        return;
    };
    let Ok(url) = web_sys::Url::new(&href) else {
        return;
    };
    match value {
        Some(value) => url.set_hash(&format!(
            "{key}={}",
            omni_api::common::encode_uri_component(value)
        )),
        None => url.set_hash(""),
    }
    replace_url(&url);
}

fn replace_url(url: &web_sys::Url) {
    let path = format!("{}{}{}", url.pathname(), url.search(), url.hash());
    if let Ok(history) = window().history() {
        let _ = history.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&path));
    }
}

/// A per-viewer preference from `localStorage`; `None` when storage is
/// missing, blocked or throws.
pub fn stored_pref(key: &str) -> Option<String> {
    window()
        .local_storage()
        .ok()
        .flatten()?
        .get_item(key)
        .ok()
        .flatten()
}

/// Saves a per-viewer preference; failures are ignored.
pub fn store_pref(key: &str, value: &str) {
    if let Ok(Some(storage)) = window().local_storage() {
        let _ = storage.set_item(key, value);
    }
}
