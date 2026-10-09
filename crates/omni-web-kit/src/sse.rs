//! `EventSource` as an async stream of named frames.
//!
//! Every open connection is registered so one `pagehide` listener can close
//! them all synchronously when the document is unloaded or enters the
//! back/forward cache. A stream left open by a departing document keeps
//! holding one of the browser's six HTTP/1.1 sockets per origin and stalls the
//! next page's loads; after `pagehide`, timers no longer run, so the close
//! cannot wait for an async task.

use std::cell::{Cell, RefCell};

use futures::channel::mpsc;
use wasm_bindgen::JsCast as _;
use wasm_bindgen::closure::Closure;

type Listener = (String, Closure<dyn FnMut(web_sys::Event)>);

/// One received server-sent event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SseMessage {
    /// A named event with its `data`.
    Event { name: String, data: String },
    /// The `error` event; `closed` when `readyState == CLOSED`.
    Error { closed: bool },
}

struct Registered {
    id: u32,
    source: web_sys::EventSource,
    sender: mpsc::UnboundedSender<SseMessage>,
}

thread_local! {
    static OPEN: RefCell<Vec<Registered>> = const { RefCell::new(Vec::new()) };
    static NEXT_ID: Cell<u32> = const { Cell::new(0) };
    static PAGEHIDE_INSTALLED: Cell<bool> = const { Cell::new(false) };
}

/// Closes every open stream and reports it as closed to its reader, which
/// then reconnects through its normal path if the page is shown again.
fn close_all() {
    let open = OPEN.with(|open| std::mem::take(&mut *open.borrow_mut()));
    for entry in open {
        entry.source.close();
        let _ = entry
            .sender
            .unbounded_send(SseMessage::Error { closed: true });
    }
}

fn install_pagehide() {
    if PAGEHIDE_INSTALLED.with(|installed| installed.replace(true)) {
        return;
    }
    let Some(window) = web_sys::window() else {
        return;
    };
    let on_pagehide = Closure::<dyn FnMut(web_sys::Event)>::new(|_| close_all());
    let _ =
        window.add_event_listener_with_callback("pagehide", on_pagehide.as_ref().unchecked_ref());
    // Lives for the page.
    on_pagehide.forget();
}

/// An open `EventSource` and the listeners feeding [`SseConnection::next`].
/// Dropping it removes the listeners and closes the connection.
pub struct SseConnection {
    id: u32,
    source: web_sys::EventSource,
    receiver: mpsc::UnboundedReceiver<SseMessage>,
    listeners: Vec<Listener>,
}

impl SseConnection {
    /// Opens `url` and listens for each name in `events` plus `error`.
    pub fn open(url: &str, events: &[&str]) -> Option<Self> {
        let source = web_sys::EventSource::new(url).ok()?;
        let (sender, receiver) = mpsc::unbounded();
        let mut listeners = Vec::new();
        for name in events {
            let tx = sender.clone();
            let event_name = (*name).to_owned();
            let closure =
                Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
                    let data = event
                        .dyn_ref::<web_sys::MessageEvent>()
                        .and_then(|m| m.data().as_string())
                        .unwrap_or_default();
                    let _ = tx.unbounded_send(SseMessage::Event {
                        name: event_name.clone(),
                        data,
                    });
                });
            let _ = source.add_event_listener_with_callback(name, closure.as_ref().unchecked_ref());
            listeners.push(((*name).to_owned(), closure));
        }
        install_pagehide();
        let id = NEXT_ID.with(|next| {
            let id = next.get();
            next.set(id.wrapping_add(1));
            id
        });
        OPEN.with(|open| {
            open.borrow_mut().push(Registered {
                id,
                source: source.clone(),
                sender: sender.clone(),
            });
        });
        let tx = sender;
        let error_source = source.clone();
        let on_error = Closure::<dyn FnMut(web_sys::Event)>::new(move |_event: web_sys::Event| {
            let closed = error_source.ready_state() == web_sys::EventSource::CLOSED;
            let _ = tx.unbounded_send(SseMessage::Error { closed });
        });
        let _ = source.add_event_listener_with_callback("error", on_error.as_ref().unchecked_ref());
        listeners.push(("error".to_owned(), on_error));
        Some(Self {
            id,
            source,
            receiver,
            listeners,
        })
    }

    /// The next frame; `None` once every listener is gone.
    pub async fn next(&mut self) -> Option<SseMessage> {
        use futures::StreamExt as _;
        self.receiver.next().await
    }
}

impl Drop for SseConnection {
    fn drop(&mut self) {
        OPEN.with(|open| open.borrow_mut().retain(|entry| entry.id != self.id));
        for (name, closure) in &self.listeners {
            let _ = self
                .source
                .remove_event_listener_with_callback(name, closure.as_ref().unchecked_ref());
        }
        self.source.close();
    }
}
