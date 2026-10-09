//! Shell chrome shared with pages: the dynamic last crumb and the phone
//! page title. A page calls [`use_page_label`] with a reactive label
//! (streamer name, workspace subject); the override is keyed by path, so a
//! stale label from the previous page never shows on the next one.

use leptos::prelude::*;

use crate::router::use_path;

#[derive(Clone, Copy)]
pub struct ChromeContext {
    label: RwSignal<Option<(String, String)>>,
}

pub fn provide_chrome() -> ChromeContext {
    let context = ChromeContext {
        label: RwSignal::new(None),
    };
    provide_context(context);
    context
}

impl ChromeContext {
    /// The page label set for `path`, if any.
    pub fn label_for(self, path: &str) -> Option<String> {
        self.label.with(|label| {
            label
                .as_ref()
                .filter(|(p, _)| p == path)
                .map(|(_, l)| l.clone())
        })
    }
}

/// Sets the last crumb (and phone title) of the current page while it is
/// mounted. Empty labels are ignored.
pub fn use_page_label(label: impl Fn() -> Option<String> + Send + Sync + 'static) {
    let Some(context) = use_context::<ChromeContext>() else {
        return;
    };
    let path = use_path();
    let mounted_path = path.get_untracked();
    Effect::new(move |_| {
        let next = label().filter(|l| !l.is_empty());
        if path.get() != mounted_path {
            return;
        }
        context.label.set(next.map(|l| (mounted_path.clone(), l)));
    });
}
