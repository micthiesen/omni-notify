//! Shared fixtures for the omni-imap integration tests.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use omni_core::clock::{SharedClock, TestClock};
use omni_core::email::{EmailLinkMetadata, ListUnsubscribe};
use omni_http::SideEffectMode;
use omni_imap::fake::FakeServer;
use omni_imap::map_message::{BodyEnricher, LinkMetadataInput};
use omni_imap::transport::{ImapTransport, TransportOptions};
use omni_testkit::TestStore;
use tokio_util::task::TaskTracker;

/// 2026-09-30T12:00:00Z.
pub const NOW_MS: i64 = 1_790_769_600_000;

/// Body enrichment stand-in: HTML text is the raw HTML, no links.
pub struct PlainEnricher;

impl BodyEnricher for PlainEnricher {
    fn html_to_text(&self, html: &str) -> String {
        html.to_owned()
    }
    fn interesting_links(&self, _html: &str) -> Vec<String> {
        Vec::new()
    }
    fn link_metadata(&self, input: LinkMetadataInput<'_>) -> EmailLinkMetadata {
        let urls: Vec<String> = input
            .header_lines
            .iter()
            .filter(|h| h.key == "list-unsubscribe")
            .flat_map(|h| {
                let line = String::from_utf8_lossy(&h.raw).into_owned();
                line.split('<')
                    .skip(1)
                    .filter_map(|part| part.split('>').next().map(str::to_owned))
                    .collect::<Vec<_>>()
            })
            .collect();
        let present = !urls.is_empty();
        EmailLinkMetadata {
            links: Vec::new(),
            links_truncated: false,
            list_unsubscribe: ListUnsubscribe {
                urls,
                post: input
                    .header_lines
                    .iter()
                    .any(|h| h.key == "list-unsubscribe-post")
                    .then(|| "List-Unsubscribe=One-Click".to_owned()),
                present,
                truncated: false,
            },
        }
    }
}

pub struct Harness {
    pub server: FakeServer,
    pub transport: ImapTransport,
    pub store: TestStore,
    pub clock: Arc<TestClock>,
    pub tracker: TaskTracker,
}

/// A transport with a fake client attached (no IDLE loop).
pub async fn harness(server: FakeServer) -> Harness {
    let clock = TestClock::new(NOW_MS);
    let shared: SharedClock = clock.clone();
    let store = TestStore::new(shared.clone()).await;
    let tracker = TaskTracker::new();
    let transport = ImapTransport::new(TransportOptions {
        connector: server.connector(),
        store: store.store.clone(),
        clock: shared,
        enricher: Arc::new(PlainEnricher),
        mode: SideEffectMode::Live,
        tracker: tracker.clone(),
    });
    transport.attach_client(Box::new(server.client())).await;
    Harness {
        server,
        transport,
        store,
        clock,
        tracker,
    }
}

/// A transport that connects through the fake connector (`start`).
pub async fn unconnected(server: FakeServer, mode: SideEffectMode) -> Harness {
    let clock = TestClock::new(NOW_MS);
    let shared: SharedClock = clock.clone();
    let store = TestStore::new(shared.clone()).await;
    let tracker = TaskTracker::new();
    let transport = ImapTransport::new(TransportOptions {
        connector: server.connector(),
        store: store.store.clone(),
        clock: shared,
        enricher: Arc::new(PlainEnricher),
        mode,
        tracker: tracker.clone(),
    });
    Harness {
        server,
        transport,
        store,
        clock,
        tracker,
    }
}

/// A minimal message source.
pub fn source(message_id: &str, subject: &str) -> Vec<u8> {
    format!(
        "Message-ID: {message_id}\r\nSubject: {subject}\r\nFrom: sender@example.test\r\nDate: Tue, 01 Sep 2026 10:00:00 +0000\r\n\r\nbody"
    )
    .into_bytes()
}
