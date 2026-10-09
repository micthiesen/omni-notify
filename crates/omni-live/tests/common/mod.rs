//! Shared test support for the omni-live integration tests.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::type_complexity)]
#![allow(dead_code)]

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use jiff::tz::TimeZone;
use omni_core::clock::{SharedClock, TestClock};
use omni_http::public::PublicHttpClient;
use omni_live::PlatformBinding;
use omni_live::error::NotifyError;
use omni_live::notify::{LiveMessage, LiveNotifier};
use omni_live::platform::FetchedStatus;
use omni_live::platforms::StatusFetcher;
use omni_testkit::TestStore;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub async fn test_store(epoch_ms: i64) -> (TestStore, Arc<TestClock>) {
    let clock = omni_testkit::test_clock(epoch_ms);
    let shared: SharedClock = clock.clone();
    (TestStore::new(shared).await, clock)
}

pub fn utc() -> TimeZone {
    TimeZone::UTC
}

/// A guarded public client whose `origins` go to the mock server.
pub fn public_client(server: &wiremock::MockServer, origins: &[&str]) -> PublicHttpClient {
    PublicHttpClient::new(&omni_testkit::mock_http(server, origins)).allow_loopback_for_tests()
}

/// A one-connection-at-a-time server answering every request with a chunked
/// body (no Content-Length), so only the streamed byte count can stop it.
pub async fn chunked_server(chunks: Vec<&'static str>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let chunks = chunks.clone();
            tokio::spawn(async move {
                let mut buffer = [0u8; 4096];
                let mut request = Vec::new();
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    match socket.read(&mut buffer).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => request.extend_from_slice(&buffer[..n]),
                    }
                }
                let mut response = String::from(
                    "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Type: text/plain\r\n\r\n",
                );
                for chunk in chunks {
                    response.push_str(&format!("{:x}\r\n{chunk}\r\n", chunk.len()));
                }
                response.push_str("0\r\n\r\n");
                let _ = socket.write_all(response.as_bytes()).await;
            });
        }
    });
    format!("http://{address}")
}

/// Records notifications; can be told to fail the next sends.
#[derive(Clone, Default)]
pub struct FakeNotifier {
    pub sent: Arc<Mutex<Vec<(Option<String>, LiveMessage)>>>,
    pub failures: Arc<Mutex<VecDeque<String>>>,
}

impl FakeNotifier {
    pub fn fail_next(&self, message: &str) {
        self.failures.lock().unwrap().push_back(message.to_owned());
    }

    pub fn sent(&self) -> Vec<(Option<String>, LiveMessage)> {
        self.sent.lock().unwrap().clone()
    }

    pub fn count(&self) -> usize {
        self.sent.lock().unwrap().len()
    }

    pub fn titles(&self) -> Vec<String> {
        self.sent().into_iter().map(|(_, m)| m.title).collect()
    }
}

impl LiveNotifier for FakeNotifier {
    fn send<'a>(
        &'a self,
        token: Option<&'a str>,
        message: LiveMessage,
    ) -> BoxFuture<'a, Result<(), NotifyError>> {
        Box::pin(async move {
            self.sent
                .lock()
                .unwrap()
                .push((token.map(str::to_owned), message));
            match self.failures.lock().unwrap().pop_front() {
                Some(error) => Err(NotifyError::new(error)),
                None => Ok(()),
            }
        })
    }
}

/// Answers per platform (or per binding), counting calls.
#[derive(Clone, Default)]
pub struct FakeFetcher {
    pub by_platform: Arc<Mutex<HashMap<omni_live::Platform, FetchedStatus>>>,
    pub calls: Arc<Mutex<Vec<PlatformBinding>>>,
}

impl FakeFetcher {
    pub fn set(&self, platform: omni_live::Platform, status: FetchedStatus) {
        self.by_platform.lock().unwrap().insert(platform, status);
    }

    pub fn calls_for(&self, platform: omni_live::Platform) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|b| b.platform == platform)
            .count()
    }
}

impl StatusFetcher for FakeFetcher {
    fn fetch<'a>(&'a self, binding: &'a PlatformBinding) -> BoxFuture<'a, FetchedStatus> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(binding.clone());
            self.by_platform
                .lock()
                .unwrap()
                .get(&binding.platform)
                .cloned()
                .unwrap_or(FetchedStatus::Offline)
        })
    }
}

/// Makes every later docstore operation fail (the `blobs` table is gone).
pub async fn break_store(store: &omni_store::Store) {
    store
        .write(|tx| {
            tx.connection()
                .execute_batch("DROP TABLE blobs")
                .map_err(|e| omni_store::StoreError::Sqlite(e.to_string()))
        })
        .await
        .unwrap();
}
