//! Pipeline fixtures for the omni-parcel integration tests.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use omni_core::clock::TestClock;
use omni_core::email::FetchedEmail;
use omni_email::activity::LlmCost;
use omni_email::triage::{
    Classified, EmailTriage, TriageClassifier, TriageEmail, TriageError, TriageVerdict,
};
use omni_http::public::PublicHttpClient;
use omni_parcel::carriers::carrier_map::CarrierDirectory;
use omni_parcel::error::ParcelError;
use omni_parcel::extraction::{
    DeliveryExtractor, ExtractDeliveriesResult, ExtractedDelivery, ExtractionEmail,
};
use omni_parcel::log_file::LogFile;
use omni_parcel::parcel_api::{ParcelSubmitter, SubmitParams, SubmitResult};
use omni_parcel::pipeline::{DeliveryPipeline, PipelineDeps};
use omni_store::Store;
use omni_tasks::{EventBus, RunLogs};
use omni_testkit::TestStore;
use serde_json::json;
use tokio_util::task::TaskTracker;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

pub const NOW: i64 = 1_800_000_000_000;

pub fn shipment(id: &str) -> FetchedEmail {
    FetchedEmail {
        id: id.to_owned(),
        origin: None,
        subject: "Shipment".to_owned(),
        from: "merchant@example.com".to_owned(),
        to: None,
        cc: None,
        reply_to: None,
        message_id: None,
        references: None,
        in_reply_to: None,
        text_body: "Tracking follows".to_owned(),
        links: Vec::new(),
        link_metadata: None,
        received_at: "2026-09-01T00:00:00Z".to_owned(),
        attachments: Vec::new(),
    }
}

pub struct YesTriage;

impl TriageClassifier for YesTriage {
    fn classify(&self, _email: TriageEmail) -> BoxFuture<'static, Result<Classified, TriageError>> {
        Box::pin(async {
            Ok(Classified {
                verdict: TriageVerdict {
                    parcel: true,
                    calendar: false,
                    reason: "tracking".to_owned(),
                },
                cost: Some(LlmCost::Cents(0.25)),
            })
        })
    }
}

type ExtractFn = Box<
    dyn Fn(&ExtractionEmail) -> BoxFuture<'static, Result<ExtractDeliveriesResult, ParcelError>>
        + Send
        + Sync,
>;

pub struct FakeExtractor(pub ExtractFn);

impl DeliveryExtractor for FakeExtractor {
    fn extract<'a>(
        &'a self,
        email: &'a ExtractionEmail,
        _log_file: Option<&'a LogFile>,
    ) -> BoxFuture<'a, Result<ExtractDeliveriesResult, ParcelError>> {
        (self.0)(email)
    }
}

pub fn deliveries(items: &[(&str, &[&str])]) -> ExtractDeliveriesResult {
    ExtractDeliveriesResult {
        deliveries: items
            .iter()
            .map(|(tracking, candidates)| ExtractedDelivery {
                tracking_number: (*tracking).to_owned(),
                carrier_candidates: candidates.iter().map(|c| (*c).to_owned()).collect(),
                description: "Camera".to_owned(),
            })
            .collect(),
        cost: LlmCost::Cents(1.0),
    }
}

pub fn always_extract(result: ExtractDeliveriesResult) -> Arc<FakeExtractor> {
    Arc::new(FakeExtractor(Box::new(move |_| {
        let result = result.clone();
        Box::pin(async move { Ok(result) })
    })))
}

/// Scripted Parcel answers (the last one repeats); records every call.
pub struct FakeSubmitter {
    pub calls: Mutex<Vec<SubmitParams>>,
    script: Mutex<VecDeque<SubmitResult>>,
}

impl FakeSubmitter {
    pub fn new(script: &[SubmitResult]) -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            script: Mutex::new(script.iter().copied().collect()),
        })
    }

    pub fn calls(&self) -> Vec<SubmitParams> {
        self.calls.lock().unwrap().clone()
    }
}

impl ParcelSubmitter for FakeSubmitter {
    fn submit<'a>(
        &'a self,
        params: &'a SubmitParams,
        _log: Option<&'a LogFile>,
    ) -> BoxFuture<'a, SubmitResult> {
        self.calls.lock().unwrap().push(params.clone());
        let mut script = self.script.lock().unwrap();
        let result = if script.len() > 1 {
            script.pop_front().unwrap_or(SubmitResult::Success)
        } else {
            script.front().copied().unwrap_or(SubmitResult::Success)
        };
        Box::pin(async move { result })
    }
}

pub struct Harness {
    pub store: TestStore,
    pub clock: Arc<TestClock>,
    pub pipeline: DeliveryPipeline,
    pub server: MockServer,
}

pub async fn harness(
    extractor: Arc<dyn DeliveryExtractor>,
    submitter: Arc<dyn ParcelSubmitter>,
    carriers: serde_json::Value,
) -> Harness {
    let server = omni_testkit::mock_server().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(carriers))
        .mount(&server)
        .await;
    let clock = TestClock::new(NOW);
    let store = TestStore::new(clock.clone()).await;
    let http = omni_testkit::mock_http(&server, &["https://api.parcel.app"]);
    let carriers = Arc::new(
        CarrierDirectory::new(
            PublicHttpClient::new(&http).allow_loopback_for_tests(),
            clock.clone(),
        )
        .unwrap(),
    );
    let pipeline = DeliveryPipeline::new(PipelineDeps {
        store: store.store.clone(),
        run_logs: RunLogs::new(EventBus::new(64), clock.clone()),
        triage: EmailTriage::new(Arc::new(YesTriage)),
        carriers,
        extractor,
        submitter,
        self_address: None,
        logs_path: None,
        tz: jiff::tz::TimeZone::UTC,
        tracker: TaskTracker::new(),
    });
    Harness {
        store,
        clock,
        pipeline,
        server,
    }
}

pub fn ups_only() -> serde_json::Value {
    json!({"ups": "UPS", "canpost": "Canada Post", "dicom": "GLS Canada"})
}

/// Fails every update of an existing `entity` row until [`heal`].
pub async fn break_updates(store: &Store, entity: &str) {
    let entity = entity.to_owned();
    store
        .write(move |tx| {
            tx.connection()
                .execute_batch(&format!(
                    "CREATE TRIGGER fail_update BEFORE UPDATE ON blobs WHEN NEW.entity = '{entity}' \
                     BEGIN SELECT RAISE(ABORT, 'crash after Parcel accepted request'); END;"
                ))
                .map_err(|e| omni_store::StoreError::Sqlite(e.to_string()))
        })
        .await
        .unwrap();
}

pub async fn heal(store: &Store) {
    store
        .write(|tx| {
            tx.connection()
                .execute_batch("DROP TRIGGER IF EXISTS fail_update;")
                .map_err(|e| omni_store::StoreError::Sqlite(e.to_string()))
        })
        .await
        .unwrap();
}
