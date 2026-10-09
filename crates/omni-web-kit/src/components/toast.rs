//! Transient status toast (`components/Toast.tsx`).

use std::time::Duration;

use leptos::prelude::*;

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

/// `useToast`: the current toast and a `show` that hides it after 4 s.
#[derive(Clone, Copy)]
pub struct ToastHandle {
    pub toast: RwSignal<Option<ToastState>>,
    timer: StoredValue<Option<TaskHandle>>,
}

impl ToastHandle {
    pub fn show(&self, message: impl Into<String>, kind: ToastKind) {
        if let Some(Some(previous)) = self.timer.try_get_value() {
            previous.abort();
        }
        self.toast.set(Some(ToastState {
            message: message.into(),
            kind,
        }));
        let toast = self.toast;
        let handle = spawn_detached(async move {
            sleep(Duration::from_secs(4)).await;
            toast.set(None);
        });
        self.timer.try_set_value(Some(handle));
    }
}

pub fn use_toast() -> ToastHandle {
    let handle = ToastHandle {
        toast: RwSignal::new(None),
        timer: StoredValue::new(None),
    };
    let timer = handle.timer;
    on_cleanup(move || {
        if let Some(Some(timer)) = timer.try_get_value() {
            timer.abort();
        }
    });
    handle
}

#[component]
pub fn Toast(#[prop(into)] toast: Signal<Option<ToastState>>) -> impl IntoView {
    move || {
        toast.get().map(|toast| {
            let (role, kind) = match toast.kind {
                ToastKind::Error => ("alert", "error"),
                ToastKind::Info => ("status", "info"),
            };
            view! { <div role=role class=format!("toast toast-{kind}")>{toast.message}</div> }
        })
    }
}
