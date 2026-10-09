//! The calendar pipeline over the real `omni_email` core
//! ([`OmniEmailSupport`]): sender rules, shared triage, activity rows, the
//! durable retry queue and per-email log capture all land in the store.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::sync::{Arc, OnceLock};

use common::{CALENDAR_URL, NoAttachments, caldav, email, extraction};
use futures::future::BoxFuture;
use omni_ai::ModelRole;
use omni_calendar::pipeline::{CalendarEventPipeline, PipelineDeps};
use omni_calendar::{Caldav, OmniEmailSupport};
use omni_core::clock::SystemClock;
use omni_email::activity::{self, AdmitTier, EmailActivityOutcome, LlmCost};
use omni_email::sender_rules::{self, RuleScope, RuleVerdict};
use omni_email::triage::{
    Classified, EmailTriage, TriageClassifier, TriageEmail, TriageError, TriageVerdict,
};
use omni_email::{activity_logs, retry};
use omni_tasks::{EventBus, RunLogLayer, RunLogs};
use omni_testkit::TestApp;
use serde_json::json;
use tracing_subscriber::layer::SubscriberExt as _;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Says "calendar" for every email, at a fixed price.
struct CalendarClassifier;

impl TriageClassifier for CalendarClassifier {
    fn classify(&self, _email: TriageEmail) -> BoxFuture<'static, Result<Classified, TriageError>> {
        Box::pin(async {
            Ok(Classified {
                verdict: TriageVerdict {
                    parcel: false,
                    calendar: true,
                    reason: "an appointment".to_owned(),
                },
                cost: Some(LlmCost::Cents(0.5)),
            })
        })
    }
}

fn pipeline(app: &TestApp, caldav: Caldav, run_logs: RunLogs) -> CalendarEventPipeline {
    CalendarEventPipeline::new(PipelineDeps {
        config: app.ctx.config.clone(),
        store: app.ctx.store.clone(),
        clock: app.ctx.clock.clone(),
        ai: app.ctx.ai.clone(),
        pushover: app.ctx.pushover.clone(),
        caldav,
        support: Arc::new(OmniEmailSupport::new(
            app.ctx.store.clone(),
            run_logs,
            EmailTriage::new(Arc::new(CalendarClassifier)),
        )),
        attachments: Arc::new(NoAttachments),
    })
}

/// One process-wide capture subscriber: callsite interest is global, so a
/// per-test scoped subscriber would race with tests on other threads.
fn run_logs() -> RunLogs {
    static LOGS: OnceLock<RunLogs> = OnceLock::new();
    LOGS.get_or_init(|| {
        let logs = RunLogs::new(EventBus::new(16), Arc::new(SystemClock));
        tracing::subscriber::set_global_default(
            tracing_subscriber::registry().with(RunLogLayer::new(logs.clone())),
        )
        .expect("no other global subscriber");
        logs
    })
    .clone()
}

#[tokio::test]
async fn sender_rules_are_read_from_the_email_core_for_the_calendar_scope() {
    let app = TestApp::new().await;
    let server = MockServer::start().await;
    let store = &app.ctx.store;
    sender_rules::upsert(
        store,
        "@blocked.example",
        RuleScope::Calendar,
        RuleVerdict::Block,
    )
    .await
    .unwrap();
    // A parcel-only block does not apply to the calendar pipeline.
    sender_rules::upsert(
        store,
        "@parcel-only.example",
        RuleScope::Parcel,
        RuleVerdict::Block,
    )
    .await
    .unwrap();
    // An explicit allow beats the built-in blacklist (`news@`).
    sender_rules::upsert(
        store,
        "news@allowed.example",
        RuleScope::Both,
        RuleVerdict::Allow,
    )
    .await
    .unwrap();
    app.ai.script(
        ModelRole::CalendarExtraction,
        vec![extraction(json!([])), extraction(json!([]))],
    );
    let p = pipeline(&app, caldav(&app, &server, Some(CALENDAR_URL)), run_logs());

    p.handle_emails(&[
        email("r-block", "clinic@blocked.example", "Appointment", "x"),
        email("r-parcel", "shop@parcel-only.example", "Appointment", "x"),
        email("r-allow", "news@allowed.example", "Appointment", "x"),
    ])
    .await
    .unwrap();

    let blocked = activity::get(store, "CalendarEvents#r-block")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(blocked.outcome, EmailActivityOutcome::Filtered);
    assert_eq!(
        blocked.detail.as_deref(),
        Some("blocked by rule @blocked.example")
    );
    // No triage call ran for this email, so its cost reads as unpriced (null).
    assert_eq!(blocked.cost_cents, LlmCost::Unpriced);

    let parcel = activity::get(store, "CalendarEvents#r-parcel")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(parcel.outcome, EmailActivityOutcome::NoMatches);
    assert_eq!(parcel.admit_tier, Some(AdmitTier::Triage));
    assert_eq!(
        parcel.admit_reason.as_deref(),
        Some("triage: an appointment")
    );

    let allowed = activity::get(store, "CalendarEvents#r-allow")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(allowed.outcome, EmailActivityOutcome::NoMatches);
    assert_eq!(allowed.admit_tier, Some(AdmitTier::Rule));
    assert_eq!(
        allowed.admit_reason.as_deref(),
        Some("allowed by rule news@allowed.example")
    );
    // Rule admission never attributes the (0.5 cent) triage cost.
    assert!(
        !matches!(allowed.cost_cents, LlmCost::Cents(c) if c >= 0.5),
        "{:?}",
        allowed.cost_cents
    );
}

#[tokio::test]
async fn discovery_failure_is_queued_in_the_durable_retry_queue() {
    let app = TestApp::new().await;
    let server = MockServer::start().await;
    Mock::given(method("PROPFIND"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let p = pipeline(&app, caldav(&app, &server, None), run_logs());

    p.handle_emails(&[email("d-1", "clinic@example.com", "Appointment", "x")])
        .await
        .unwrap();

    let reason = "calendar discovery failed: CalDAV PROPFIND failed: 503 Service Unavailable (https://caldav.icloud.com/)\n";
    let queued = retry::get(&app.ctx.store, "CalendarEvents#d-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(queued.pipeline, "CalendarEvents");
    assert_eq!(queued.reason, reason);
    let row = activity::get(&app.ctx.store, "CalendarEvents#d-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.outcome, EmailActivityOutcome::Error);
    assert_eq!(row.detail.as_deref(), Some(reason));
    assert_eq!(row.cost_cents, LlmCost::Cents(0.5));
}

#[tokio::test]
async fn processed_emails_record_activity_and_their_captured_log() {
    let app = TestApp::new().await;
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(201))
        .mount(&server)
        .await;
    app.ai.script(
        ModelRole::CalendarExtraction,
        vec![extraction(json!([
            { "action": "create", "title": "🦷 Dentist", "startDate": "2026-09-03", "startTime": "09:00", "allDay": false }
        ]))],
    );
    let p = pipeline(&app, caldav(&app, &server, Some(CALENDAR_URL)), run_logs());

    p.handle_emails(&[email("p-1", "clinic@example.com", "Appointment", "x")])
        .await
        .unwrap();

    let row = activity::get(&app.ctx.store, "CalendarEvents#p-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.outcome, EmailActivityOutcome::Processed);
    assert_eq!(
        row.items.as_deref().unwrap(),
        ["\"🦷 Dentist\" on 2026-09-03: created"]
    );
    assert_eq!(row.received_at, 1_788_220_800_000);
    let captured = activity_logs::get(&app.ctx.store, "CalendarEvents#p-1")
        .await
        .unwrap()
        .expect("captured log row");
    let messages: Vec<&str> = captured.lines.iter().map(|l| l.msg.as_str()).collect();
    assert!(
        messages
            .iter()
            .any(|m| m.starts_with("Extracting events from: \"Appointment\"")),
        "{messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|m| m.starts_with("Created: \"🦷 Dentist\"")),
        "{messages:?}"
    );
    assert!(retry::get_all(&app.ctx.store).await.unwrap().is_empty());
}
