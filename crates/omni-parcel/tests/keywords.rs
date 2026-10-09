//! Port of `src/parcel-tracker/filter/keywords.spec.ts`. `EMAIL_SELF_ADDRESS`
//! is passed through `FilterDeps` instead of mutating global config; the
//! carrier list comes from a local mock that serves no carriers.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use omni_core::clock::TestClock;
use omni_email::activity::AdmitTier;
use omni_email::sender_rules::{self, RuleScope, RuleVerdict};
use omni_email::triage::{
    Classified, EmailTriage, TriageClassifier, TriageEmail, TriageError, TriageVerdict,
};
use omni_http::public::PublicHttpClient;
use omni_parcel::carriers::carrier_map::CarrierDirectory;
use omni_parcel::filter::{
    FilterDeps, FilterResult, filter_tracking_candidate, is_aliexpress_order_status,
};
use omni_store::Store;
use omni_testkit::TestStore;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

struct StubTriage {
    verdict: Option<TriageVerdict>,
    calls: Mutex<usize>,
}

impl TriageClassifier for StubTriage {
    fn classify(&self, email: TriageEmail) -> BoxFuture<'static, Result<Classified, TriageError>> {
        *self.calls.lock().unwrap() += 1;
        let result = match &self.verdict {
            Some(verdict) => Ok(Classified {
                verdict: verdict.clone(),
                cost: None,
            }),
            None => Err(TriageError {
                email_id: email.id,
                message: "model down".to_owned(),
            }),
        };
        Box::pin(async move { result })
    }
}

fn stub(verdict: Option<TriageVerdict>) -> (EmailTriage, Arc<StubTriage>) {
    let classifier = Arc::new(StubTriage {
        verdict,
        calls: Mutex::new(0),
    });
    (EmailTriage::new(classifier.clone()), classifier)
}

fn parcel_yes() -> Option<TriageVerdict> {
    Some(TriageVerdict {
        parcel: true,
        calendar: false,
        reason: "tracking".to_owned(),
    })
}

fn parcel_no() -> Option<TriageVerdict> {
    Some(TriageVerdict {
        parcel: false,
        calendar: false,
        reason: "no shipment".to_owned(),
    })
}

struct Fixture {
    store: TestStore,
    carriers: CarrierDirectory,
    _server: MockServer,
}

async fn fixture() -> Fixture {
    let server = omni_testkit::mock_server().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
        .mount(&server)
        .await;
    let http = omni_testkit::mock_http(&server, &["https://api.parcel.app"]);
    let clock = TestClock::new(1_800_000_000_000);
    Fixture {
        store: TestStore::new(clock.clone()).await,
        carriers: CarrierDirectory::new(
            PublicHttpClient::new(&http).allow_loopback_for_tests(),
            clock,
        )
        .unwrap(),
        _server: server,
    }
}

static NEXT_ID: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn make(from: &str, subject: &str, text_body: &str) -> TriageEmail {
    TriageEmail {
        id: format!(
            "email-{}",
            NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ),
        subject: subject.to_owned(),
        from: from.to_owned(),
        text_body: text_body.to_owned(),
        links: Vec::new(),
    }
}

async fn filter(
    fx: &Fixture,
    triage: &EmailTriage,
    email: TriageEmail,
    self_address: Option<&str>,
) -> FilterResult {
    filter_tracking_candidate(
        &FilterDeps {
            store: &fx.store.store,
            triage,
            carriers: &fx.carriers,
            self_address,
        },
        &email,
    )
    .await
    .unwrap()
}

async fn rule(store: &Store, pattern: &str, scope: RuleScope, verdict: RuleVerdict) {
    sender_rules::upsert(store, pattern, scope, verdict)
        .await
        .unwrap();
}

fn pass(reason: &str, admit_tier: AdmitTier) -> FilterResult {
    FilterResult::Pass {
        reason: reason.to_owned(),
        admit_tier,
    }
}

fn skip(reason: &str) -> FilterResult {
    FilterResult::Skip {
        reason: reason.to_owned(),
    }
}

#[tokio::test]
async fn a_block_rule_beats_even_a_carrier_sender() {
    let fx = fixture().await;
    rule(
        &fx.store.store,
        "ups.com",
        RuleScope::Parcel,
        RuleVerdict::Block,
    )
    .await;
    let (triage, _) = stub(parcel_yes());
    let result = filter(
        &fx,
        &triage,
        make("noreply@ups.com", "Delivery update", ""),
        None,
    )
    .await;
    assert_eq!(result, skip("blocked by rule ups.com"));
}

#[tokio::test]
async fn an_allow_rule_passes_without_consulting_triage() {
    let fx = fixture().await;
    rule(
        &fx.store.store,
        "somestore.com",
        RuleScope::Both,
        RuleVerdict::Allow,
    )
    .await;
    let (triage, calls) = stub(None);
    let result = filter(
        &fx,
        &triage,
        make("orders@somestore.com", "Anything at all", ""),
        None,
    )
    .await;
    assert_eq!(
        result,
        pass("allowed by rule somestore.com", AdmitTier::Rule)
    );
    assert_eq!(*calls.calls.lock().unwrap(), 0);
}

#[tokio::test]
async fn calendar_scoped_rules_do_not_affect_the_parcel_filter() {
    let fx = fixture().await;
    rule(
        &fx.store.store,
        "ups.com",
        RuleScope::Calendar,
        RuleVerdict::Block,
    )
    .await;
    let (triage, _) = stub(None);
    let result = filter(
        &fx,
        &triage,
        make("noreply@ups.com", "Delivery update", ""),
        None,
    )
    .await;
    assert_eq!(result, pass("carrier sender", AdmitTier::Builtin));
}

#[tokio::test]
async fn an_allow_rule_overrides_the_built_in_blacklist() {
    let fx = fixture().await;
    rule(
        &fx.store.store,
        "npmjs.com",
        RuleScope::Parcel,
        RuleVerdict::Allow,
    )
    .await;
    let (triage, _) = stub(None);
    let result = filter(
        &fx,
        &triage,
        make("support@npmjs.com", "Successfully published a package", ""),
        None,
    )
    .await;
    assert_eq!(result, pass("allowed by rule npmjs.com", AdmitTier::Rule));
}

#[tokio::test]
async fn rejects_blacklisted_amazon_senders_and_subdomains() {
    let fx = fixture().await;
    let (triage, _) = stub(parcel_yes());
    for from in ["shipment-tracking@amazon.com", "ship-confirm@amazon.co.uk"] {
        let result = filter(&fx, &triage, make(from, "Your order has shipped", ""), None).await;
        assert_eq!(
            result,
            skip("blacklisted sender"),
            "expected {from} to be rejected"
        );
    }
}

#[tokio::test]
async fn rejects_npm_registry_mail() {
    let fx = fixture().await;
    let (triage, _) = stub(parcel_yes());
    let result = filter(
        &fx,
        &triage,
        make(
            "support@npmjs.com",
            "Successfully published your-package@1.0.0",
            "",
        ),
        None,
    )
    .await;
    assert_eq!(result, skip("blacklisted sender"));
}

#[tokio::test]
async fn rejects_the_users_own_outgoing_address_when_configured() {
    let fx = fixture().await;
    let (triage, _) = stub(parcel_yes());
    let result = filter(
        &fx,
        &triage,
        make("michael@example.com", "Fwd: your package shipped", ""),
        Some("michael@example.com"),
    )
    .await;
    assert_eq!(result, skip("blacklisted sender"));
}

#[tokio::test]
async fn rejects_food_delivery_senders_even_with_tracking_keywords() {
    let fx = fixture().await;
    let (triage, _) = stub(parcel_yes());
    let result = filter(
        &fx,
        &triage,
        make("noreply@uber.com", "Your delivery is in transit", ""),
        None,
    )
    .await;
    assert_eq!(result, skip("blacklisted sender"));
}

#[test]
fn aliexpress_subject_order_status_cases() {
    for (subject, expected) in [
        ("Order 8196234512: view details", true),
        ("Your order is awaiting confirmation", true),
        ("Order shipped! See what's on the way", true),
        ("Order confirmed — thanks for shopping", true),
        ("Delivery update for your order", true),
        ("Awaiting Payment: complete your purchase", true),
        ("Package from your order is ready to ship", false),
    ] {
        assert_eq!(
            is_aliexpress_order_status("transaction@notice.aliexpress.com", subject),
            expected,
            "aliexpress subject {subject:?}"
        );
    }
}

#[test]
fn never_matches_non_aliexpress_senders() {
    assert!(!is_aliexpress_order_status(
        "orders@shop.com",
        "Order shipped"
    ));
}

#[tokio::test]
async fn skips_order_status_emails_without_consulting_triage() {
    let fx = fixture().await;
    let (triage, calls) = stub(parcel_yes());
    let result = filter(
        &fx,
        &triage,
        make(
            "transaction@notice.aliexpress.com",
            "Order 8196234512: Awaiting delivery",
            "",
        ),
        None,
    )
    .await;
    assert_eq!(result, skip("aliexpress order-status"));
    assert_eq!(*calls.calls.lock().unwrap(), 0);
}

#[tokio::test]
async fn still_lets_ready_to_ship_emails_through_to_triage() {
    let fx = fixture().await;
    let (triage, _) = stub(parcel_yes());
    let result = filter(
        &fx,
        &triage,
        make(
            "transaction@notice.aliexpress.com",
            "Package from your order is ready to ship",
            "",
        ),
        None,
    )
    .await;
    assert_eq!(result, pass("triage: tracking", AdmitTier::Triage));
}

#[tokio::test]
async fn passes_carrier_domains_without_consulting_triage() {
    let fx = fixture().await;
    let (triage, calls) = stub(None);
    let result = filter(&fx, &triage, make("noreply@FedEx.com", "Update", ""), None).await;
    assert_eq!(result, pass("carrier sender", AdmitTier::Builtin));
    assert_eq!(*calls.calls.lock().unwrap(), 0);
}

#[tokio::test]
async fn passes_when_triage_says_parcel() {
    let fx = fixture().await;
    let (triage, _) = stub(parcel_yes());
    let result = filter(
        &fx,
        &triage,
        make("orders@somestore.com", "Order update", ""),
        None,
    )
    .await;
    assert_eq!(result, pass("triage: tracking", AdmitTier::Triage));
}

#[tokio::test]
async fn fails_when_triage_says_no_even_with_tracking_keywords_present() {
    let fx = fixture().await;
    let (triage, _) = stub(parcel_no());
    let result = filter(
        &fx,
        &triage,
        make("orders@somestore.com", "Your order has shipped!", ""),
        None,
    )
    .await;
    assert_eq!(result, skip("triage: no shipment"));
}

#[tokio::test]
async fn matches_tracking_keywords_in_the_subject() {
    let fx = fixture().await;
    let (triage, _) = stub(None);
    let result = filter(
        &fx,
        &triage,
        make("orders@somestore.com", "Your order has shipped!", ""),
        None,
    )
    .await;
    assert_eq!(
        result,
        pass(
            "keyword \"shipped\" (triage unavailable)",
            AdmitTier::KeywordFallback
        )
    );
}

#[tokio::test]
async fn matches_tracking_keywords_in_the_body_case_insensitively() {
    let fx = fixture().await;
    let (triage, _) = stub(None);
    let result = filter(
        &fx,
        &triage,
        make(
            "orders@somestore.com",
            "Order confirmation",
            "Your TRACKING number is X",
        ),
        None,
    )
    .await;
    assert!(matches!(result, FilterResult::Pass { .. }));
}

#[tokio::test]
async fn rejects_unrelated_emails() {
    let fx = fixture().await;
    let (triage, _) = stub(None);
    let result = filter(
        &fx,
        &triage,
        make(
            "hello@example.com",
            "Weekly digest",
            "Here are this week's top stories.",
        ),
        None,
    )
    .await;
    assert_eq!(result, skip("no keyword match (triage unavailable)"));
}
