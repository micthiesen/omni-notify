//! One global toast region. [`provide_toasts`] (called by the shell) backs
//! every [`use_toast`] handle; info toasts hide after 4 s, errors stay until
//! dismissed. Without a region (tests), handles fall back to a local toast
//! rendered by [`Toast`].

use std::time::Duration;

use leptos::prelude::*;

use super::icon::{Glyph, Icon, IconSize};
use crate::task::{TaskHandle, sleep, spawn_detached};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToastState {
    pub message: String,
    pub kind: ToastKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ToastItem {
    id: u64,
    state: ToastState,
}

/// The shell's toast list.
#[derive(Clone, Copy)]
pub struct ToastRegionContext {
    items: RwSignal<Vec<ToastItem>>,
    next_id: StoredValue<u64>,
}

const AUTO_HIDE: Duration = Duration::from_secs(4);
const MAX_VISIBLE: usize = 3;

impl ToastRegionContext {
    fn push(self, state: ToastState) {
        let id = self.next_id.get_value();
        self.next_id.set_value(id + 1);
        let kind = state.kind;
        self.items.update(|items| {
            items.retain(|item| item.state != state);
            items.push(ToastItem { id, state });
            let overflow = items.len().saturating_sub(MAX_VISIBLE);
            items.drain(..overflow);
        });
        if kind == ToastKind::Info {
            let items = self.items;
            spawn_detached(async move {
                sleep(AUTO_HIDE).await;
                items.try_update(|items| items.retain(|item| item.id != id));
            });
        }
    }

    fn dismiss(self, id: u64) {
        self.items
            .update(|items| items.retain(|item| item.id != id));
    }
}

/// Creates the global region context; render [`ToastRegion`] once.
pub fn provide_toasts() -> ToastRegionContext {
    let context = ToastRegionContext {
        items: RwSignal::new(Vec::new()),
        next_id: StoredValue::new(0),
    };
    provide_context(context);
    context
}

/// Bottom-right (above the tab bar on phone) toast stack.
#[component]
pub fn ToastRegion() -> impl IntoView {
    let Some(context) = use_context::<ToastRegionContext>() else {
        return ().into_any();
    };
    view! {
        <div class="toast-region">
            <For
                each=move || context.items.get()
                key=|item| item.id
                children=move |item| {
                    let id = item.id;
                    let fault = item.state.kind == ToastKind::Error;
                    view! {
                        <div class=if fault { "toast fault" } else { "toast" } role=if fault { "alert" } else { "status" }>
                            <span class=if fault { "status fault dot-only" } else { "status ok dot-only" } aria-hidden="true"></span>
                            <span class="toast-text">{item.state.message}</span>
                            <button
                                type="button"
                                class="btn ghost sm icon-only"
                                aria-label="Dismiss"
                                on:click=move |_| context.dismiss(id)
                            >
                                <Glyph icon=Icon::Close size=IconSize::Small/>
                            </button>
                        </div>
                    }
                }
            />
        </div>
    }
    .into_any()
}

/// A toast sender. `toast` holds the local fallback state.
#[derive(Clone, Copy)]
pub struct ToastHandle {
    pub toast: RwSignal<Option<ToastState>>,
    timer: StoredValue<Option<TaskHandle>>,
    region: Option<ToastRegionContext>,
}

impl ToastHandle {
    pub fn show(&self, message: impl Into<String>, kind: ToastKind) {
        let state = ToastState {
            message: message.into(),
            kind,
        };
        if let Some(region) = self.region {
            region.push(state);
            return;
        }
        if let Some(Some(previous)) = self.timer.try_get_value() {
            previous.abort();
        }
        self.toast.set(Some(state));
        let toast = self.toast;
        let handle = spawn_detached(async move {
            sleep(AUTO_HIDE).await;
            toast.try_set(None);
        });
        self.timer.try_set_value(Some(handle));
    }
}

pub fn use_toast() -> ToastHandle {
    let handle = ToastHandle {
        toast: RwSignal::new(None),
        timer: StoredValue::new(None),
        region: use_context::<ToastRegionContext>(),
    };
    let timer = handle.timer;
    on_cleanup(move || {
        if let Some(Some(timer)) = timer.try_get_value() {
            timer.abort();
        }
    });
    handle
}

/// Local fallback toast; renders nothing when the shell region exists.
#[component]
pub fn Toast(#[prop(into)] toast: Signal<Option<ToastState>>) -> impl IntoView {
    let has_region = use_context::<ToastRegionContext>().is_some();
    move || {
        if has_region {
            return None;
        }
        toast.get().map(|toast| {
            let fault = toast.kind == ToastKind::Error;
            view! {
                <div class="toast-region">
                    <div class=if fault { "toast fault" } else { "toast" } role=if fault { "alert" } else { "status" }>
                        <span class="toast-text">{toast.message}</span>
                    </div>
                </div>
            }
        })
    }
}
