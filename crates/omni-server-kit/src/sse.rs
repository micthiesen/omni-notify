//! Server-sent events helpers.

use std::convert::Infallible;
use std::future::Future;
use std::time::Duration;

use axum::response::sse::Event;
use futures::{Stream, StreamExt as _};
use omni_core::clock::SharedClock;

/// Headers every SSE response carries (nginx must not buffer).
pub const HEADERS: [(&str, &str); 1] = [("x-accel-buffering", "no")];

/// `event: <name>`, `id: <id>`, `data: <data>`.
pub fn event(name: &str, id: u64, data: &str) -> Event {
    Event::default().event(name).id(id.to_string()).data(data)
}

/// One `snapshot` frame (`SseSnapshotFrame`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotFrame {
    pub data: String,
    pub id: u64,
}

impl SnapshotFrame {
    /// `event: snapshot`, `id: <id>`, `data: <json>`.
    pub fn to_event(&self) -> Event {
        event("snapshot", self.id, &self.data)
    }
}

/// `enqueueInitialSnapshotFrame`: builds a new client's snapshot and
/// enqueues it while holding `lock`, the same lock every broadcast takes, so
/// a newer broadcast can never be overwritten by an older initial frame.
/// `enqueue` should register the client and queue the frame in one step.
pub async fn enqueue_initial_snapshot_frame<T, E, Fut>(
    lock: &tokio::sync::Mutex<()>,
    build_snapshot: impl FnOnce() -> Fut,
    next_id: impl FnOnce() -> u64,
    enqueue: impl FnOnce(SnapshotFrame),
) -> Result<(), E>
where
    Fut: Future<Output = Result<T, E>>,
    T: serde::Serialize,
    E: From<serde_json::Error>,
{
    let _held = lock.lock().await;
    let snapshot = build_snapshot().await?;
    let data = serde_json::to_string(&snapshot)?;
    enqueue(SnapshotFrame {
        data,
        id: next_id(),
    });
    Ok(())
}

/// Interleaves `event: ping` frames (data: epoch ms, no id) on a fixed
/// `every` cadence, independent of other frames, until `stream` ends.
pub fn with_ping(
    stream: impl Stream<Item = Event> + Send + 'static,
    every: Duration,
    clock: SharedClock,
) -> impl Stream<Item = Result<Event, Infallible>> + Send + 'static {
    let stream = Box::pin(stream);
    let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + every, every);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    futures::stream::unfold((stream, ticker), move |(mut stream, mut ticker)| {
        let clock = clock.clone();
        async move {
            tokio::select! {
                next = stream.next() => next.map(|event| (event, (stream, ticker))),
                _ = ticker.tick() => {
                    Some((ping(&clock), (stream, ticker)))
                }
            }
        }
    })
    .map(Ok)
}

/// `event: ping`, `data: <epoch ms>`, no id.
pub fn ping(clock: &SharedClock) -> Event {
    Event::default()
        .event("ping")
        .data(clock.now_ms().to_string())
}

/// [`with_ping`] that also sends one ping before anything else, as TS's
/// `Effect.repeat(..., Schedule.spaced(...))` heartbeat does on connect.
pub fn with_ping_immediately(
    stream: impl Stream<Item = Event> + Send + 'static,
    every: Duration,
    clock: SharedClock,
) -> impl Stream<Item = Result<Event, Infallible>> + Send + 'static {
    let first = ping(&clock);
    futures::stream::once(std::future::ready(Ok(first))).chain(with_ping(stream, every, clock))
}

#[cfg(test)]
mod tests {
    use super::*;
    use omni_core::clock::TestClock;
    use std::sync::{Arc, Mutex};

    /// Port of `sse.spec.ts` "prevents a newer broadcast from being
    /// overwritten by the initial frame". The `awaitSseWriter` case is
    /// dropped: axum drives the event stream, so a failed socket write drops
    /// the stream and its subscriptions by ownership.
    #[tokio::test]
    async fn prevents_a_newer_broadcast_from_being_overwritten_by_the_initial_frame() {
        #[derive(serde::Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Snapshot {
            live_channel: String,
        }

        let lock = Arc::new(tokio::sync::Mutex::new(()));
        let frames: Arc<Mutex<Vec<SnapshotFrame>>> = Arc::default();
        let live_channel = Arc::new(Mutex::new("first".to_owned()));
        let registered = Arc::new(Mutex::new(false));
        let ids = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let (started_tx, started_rx) = tokio::sync::oneshot::channel::<()>();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();

        let initial = {
            let (lock, frames, live, registered, ids) = (
                lock.clone(),
                frames.clone(),
                live_channel.clone(),
                registered.clone(),
                ids.clone(),
            );
            tokio::spawn(async move {
                enqueue_initial_snapshot_frame::<_, serde_json::Error, _>(
                    &lock,
                    || async move {
                        let snapshot = Snapshot {
                            live_channel: live.lock().unwrap().clone(),
                        };
                        started_tx.send(()).unwrap();
                        release_rx.await.unwrap();
                        Ok(snapshot)
                    },
                    || ids.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
                    |frame| {
                        *registered.lock().unwrap() = true;
                        frames.lock().unwrap().push(frame);
                    },
                )
                .await
                .unwrap();
            })
        };
        started_rx.await.unwrap();
        *live_channel.lock().unwrap() = "second".to_owned();
        let broadcast = {
            let (lock, frames, live, registered, ids) = (
                lock.clone(),
                frames.clone(),
                live_channel.clone(),
                registered.clone(),
                ids.clone(),
            );
            tokio::spawn(async move {
                let _held = lock.lock().await;
                if *registered.lock().unwrap() {
                    let data = serde_json::to_string(&Snapshot {
                        live_channel: live.lock().unwrap().clone(),
                    })
                    .unwrap();
                    frames.lock().unwrap().push(SnapshotFrame {
                        data,
                        id: ids.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
                    });
                }
            })
        };
        tokio::task::yield_now().await;
        release_tx.send(()).unwrap();
        initial.await.unwrap();
        broadcast.await.unwrap();
        assert_eq!(
            *frames.lock().unwrap(),
            vec![
                SnapshotFrame {
                    data: r#"{"liveChannel":"first"}"#.to_owned(),
                    id: 0,
                },
                SnapshotFrame {
                    data: r#"{"liveChannel":"second"}"#.to_owned(),
                    id: 1,
                },
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn pings_while_idle_and_ends_with_source() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Event>();
        let source =
            futures::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|e| (e, rx)) });
        let mut stream = Box::pin(with_ping(
            source,
            Duration::from_secs(25),
            TestClock::new(7),
        ));
        assert!(stream.next().await.is_some(), "ping after 25 s");
        assert!(tx.send(event("snapshot", 1, "{}")).is_ok());
        assert!(stream.next().await.is_some());
        drop(tx);
        assert!(stream.next().await.is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn pings_once_before_the_source_then_on_the_cadence() {
        let source =
            futures::stream::iter([event("snapshot", 0, "{}")]).chain(futures::stream::pending());
        let mut stream = Box::pin(with_ping_immediately(
            source,
            Duration::from_secs(25),
            TestClock::new(7),
        ));
        let started = tokio::time::Instant::now();
        let frames = [
            stream.next().await.unwrap().unwrap(),
            stream.next().await.unwrap().unwrap(),
            stream.next().await.unwrap().unwrap(),
        ];
        let rendered: Vec<String> = frames.iter().map(|e| format!("{e:?}")).collect();
        assert!(rendered[0].contains("ping"), "{rendered:?}");
        assert!(rendered[1].contains("snapshot"), "{rendered:?}");
        assert!(rendered[2].contains("ping"), "{rendered:?}");
        assert_eq!(started.elapsed(), Duration::from_secs(25));
    }
}
