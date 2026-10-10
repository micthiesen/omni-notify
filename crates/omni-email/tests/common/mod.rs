//! Shared fixtures for the omni-email integration tests.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use omni_core::clock::{SharedClock, TestClock};
use omni_core::email::FetchedEmail;
use omni_store::Store;
use omni_testkit::TestStore;

pub const NOW: i64 = 1_800_000_000_000;

pub async fn store_at(now: i64) -> (TestStore, Arc<TestClock>) {
    let clock = TestClock::new(now);
    let shared: SharedClock = clock.clone();
    (TestStore::new(shared).await, clock)
}

pub fn email(id: &str) -> FetchedEmail {
    FetchedEmail {
        id: id.to_owned(),
        origin: None,
        subject: "Shipment".to_owned(),
        from: "shop@example.com".to_owned(),
        to: None,
        cc: None,
        reply_to: None,
        message_id: None,
        references: None,
        in_reply_to: None,
        text_body: "On its way".to_owned(),
        links: Vec::new(),
        link_metadata: None,
        received_at: "1970-01-01T00:00:00.000Z".to_owned(),
        attachments: Vec::new(),
    }
}

/// Makes every write of `entity` rows fail inside SQLite (fault injection for
/// "persistence unavailable" cases), until [`heal`] drops the triggers.
pub async fn break_writes(store: &Store, entity: &str) {
    let entity = entity.to_owned();
    store
        .write(move |tx| {
            let conn = tx.connection();
            for (name, event) in [("insert", "INSERT"), ("update", "UPDATE")] {
                conn.execute_batch(&format!(
                    "CREATE TRIGGER fail_{name} BEFORE {event} ON blobs \
                     WHEN NEW.entity = '{entity}' \
                     BEGIN SELECT RAISE(ABORT, 'database unavailable'); END;"
                ))
                .map_err(|e| omni_store::StoreError::Sqlite(e.to_string()))?;
            }
            Ok::<_, omni_store::StoreError>(())
        })
        .await
        .unwrap();
}

pub async fn heal(store: &Store) {
    store
        .write(|tx| {
            tx.connection()
                .execute_batch(
                    "DROP TRIGGER IF EXISTS fail_insert; DROP TRIGGER IF EXISTS fail_update;",
                )
                .map_err(|e| omni_store::StoreError::Sqlite(e.to_string()))
        })
        .await
        .unwrap();
}

use futures::future::BoxFuture;
use omni_core::email::EmailHandler;
use omni_runtime::ports::{
    EmailReader, EmailReaderHealth, EmailRetryHandlers, EmailSearch, PortError,
};
use std::collections::HashMap;
use std::sync::Mutex;

type FetchFn = Box<dyn Fn(&str) -> Result<Option<FetchedEmail>, PortError> + Send + Sync>;

/// An `EmailReader` port answering `fetch_by_id` from a closure.
pub struct FakeReader {
    pub fetch: FetchFn,
    /// Every `fetch_by_id` call as `(id, fresh)`.
    pub fetches: Mutex<Vec<(String, bool)>>,
    pub searches: Mutex<Vec<EmailSearch>>,
    pub search_results: Vec<FetchedEmail>,
    pub search_available: bool,
}

impl FakeReader {
    pub fn new(
        fetch: impl Fn(&str) -> Result<Option<FetchedEmail>, PortError> + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            fetch: Box::new(fetch),
            fetches: Mutex::new(Vec::new()),
            searches: Mutex::new(Vec::new()),
            search_results: Vec::new(),
            search_available: true,
        })
    }
}

impl EmailReader for FakeReader {
    fn fetch_by_id<'a>(
        &'a self,
        id: &'a str,
        fresh: bool,
    ) -> BoxFuture<'a, Result<Option<FetchedEmail>, PortError>> {
        self.fetches.lock().unwrap().push((id.to_owned(), fresh));
        let result = (self.fetch)(id);
        Box::pin(async move { result })
    }

    fn search<'a>(
        &'a self,
        q: &'a EmailSearch,
    ) -> BoxFuture<'a, Result<Vec<FetchedEmail>, PortError>> {
        self.searches.lock().unwrap().push(q.clone());
        let results = self.search_results.clone();
        Box::pin(async move { Ok(results) })
    }

    fn health(&self) -> EmailReaderHealth {
        EmailReaderHealth {
            transport: "IMAP".to_owned(),
            search_available: self.search_available,
            drafts_available: true,
        }
    }

    fn download_attachment<'a>(
        &'a self,
        _attachment: &'a omni_core::email::EmailAttachment,
    ) -> BoxFuture<'a, Result<Option<omni_core::email::DownloadedAttachment>, PortError>> {
        Box::pin(async { Ok(None) })
    }
}

/// An `EmailRetryHandlers` port over a fixed map.
pub struct FakeHandlers(pub HashMap<String, Arc<dyn EmailHandler>>);

impl EmailRetryHandlers for FakeHandlers {
    fn handler(&self, pipeline: &str) -> Option<Arc<dyn EmailHandler>> {
        self.0.get(pipeline).cloned()
    }
}

pub fn handlers(entries: Vec<(&str, Arc<dyn EmailHandler>)>) -> Arc<FakeHandlers> {
    Arc::new(FakeHandlers(
        entries
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect(),
    ))
}

type BatchFn = Box<
    dyn Fn(Vec<FetchedEmail>) -> BoxFuture<'static, Result<(), omni_core::email::HandlerError>>
        + Send
        + Sync,
>;

/// A handler running a closure per batch.
pub struct FnHandler {
    name: &'static str,
    run: BatchFn,
}

pub fn fn_handler<F, Fut>(name: &'static str, f: F) -> Arc<FnHandler>
where
    F: Fn(Vec<FetchedEmail>) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<(), omni_core::email::HandlerError>> + Send + 'static,
{
    Arc::new(FnHandler {
        name,
        run: Box::new(move |emails| Box::pin(f(emails))),
    })
}

impl EmailHandler for FnHandler {
    fn name(&self) -> &'static str {
        self.name
    }

    fn handle<'a>(
        &'a self,
        emails: &'a [FetchedEmail],
    ) -> BoxFuture<'a, Result<(), omni_core::email::HandlerError>> {
        (self.run)(emails.to_vec())
    }
}
