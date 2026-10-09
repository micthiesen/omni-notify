//! `EventSource` as an async stream of named frames.

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

/// An open `EventSource` and the listeners feeding [`SseConnection::next`].
/// Dropping it removes the listeners and closes the connection.
pub struct SseConnection {
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
        let tx = sender;
        let error_source = source.clone();
        let on_error = Closure::<dyn FnMut(web_sys::Event)>::new(move |_event: web_sys::Event| {
            let closed = error_source.ready_state() == web_sys::EventSource::CLOSED;
            let _ = tx.unbounded_send(SseMessage::Error { closed });
        });
        let _ = source.add_event_listener_with_callback("error", on_error.as_ref().unchecked_ref());
        listeners.push(("error".to_owned(), on_error));
        Some(Self {
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
        for (name, closure) in &self.listeners {
            let _ = self
                .source
                .remove_event_listener_with_callback(name, closure.as_ref().unchecked_ref());
        }
        self.source.close();
    }
}
