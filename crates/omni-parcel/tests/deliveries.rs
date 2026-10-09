//! Parcel delivery status: the response decoder on a recorded (redacted)
//! fixture, the read client against a local mock, the budgeted
//! `ParcelDeliveries` task, `GET /api/parcels`, and the MCP tools, which only
//! read the cache.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use omni_api::parcels::{ParcelDeliveryStatus, ParcelsResponse};
use omni_core::clock::TestClock;
use omni_http::SideEffectMode;
use omni_http::public::PublicHttpClient;
use omni_mcp_kit::registry::standalone_context;
use omni_mcp_kit::{McpTool, ToolOutput};
use omni_parcel::carriers::carrier_map::CarrierDirectory;
use omni_parcel::deliveries::api::{DeliveriesClient, DeliveriesError, decode_deliveries};
use omni_parcel::deliveries::state::{self, ReadState, SkipReason};
use omni_parcel::deliveries::task::{DeliveriesTask, DeliveriesTaskDeps, TickOutcome};
use omni_parcel::persistence::{self, DeliveryAttempt, SubmissionStatus};
use omni_store::Store;
use omni_testkit::TestStore;
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const FIXTURE: &str = include_str!("fixtures/deliveries-recent.json");
const NOW: i64 = 1_790_000_000_000;
const MINUTE: i64 = 60_000;
const HOUR: i64 = 60 * MINUTE;

fn carriers_body() -> Value {
    json!({"uniuni": "UniUni", "aliex": "AliExpress", "amzlca": "Amazon Canada"})
}

struct Harness {
    clock: Arc<TestClock>,
    store: TestStore,
    server: MockServer,
    task: DeliveriesTask,
}

impl Harness {
    async fn new(mode: SideEffectMode) -> Self {
        let clock = TestClock::new(NOW);
        let store = TestStore::new(clock.clone()).await;
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/external/supported_carriers.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(carriers_body()))
            .mount(&server)
            .await;
        let http = omni_testkit::mock_http(&server, &["https://api.parcel.app"]);
        let task = DeliveriesTask::new(DeliveriesTaskDeps {
            store: store.store.clone(),
            client: DeliveriesClient::new(http.clone(), "parcel-key".to_owned()).unwrap(),
            carriers: Arc::new(
                CarrierDirectory::new(
                    PublicHttpClient::new(&http).allow_loopback_for_tests(),
                    clock.clone(),
                )
                .unwrap(),
            ),
            clock: clock.clone(),
            tracker: tokio_util::task::TaskTracker::new(),
            mode,
            tz: jiff::tz::TimeZone::UTC,
        })
        .unwrap();
        Self {
            clock,
            store,
            server,
            task,
        }
    }

    async fn respond(&self, template: ResponseTemplate) {
        self.server.reset().await;
        Mock::given(method("GET"))
            .and(path("/external/supported_carriers.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(carriers_body()))
            .mount(&self.server)
            .await;
        Mock::given(method("GET"))
            .and(path("/external/deliveries/"))
            .and(query_param("filter_mode", "recent"))
            .and(header("api-key", "parcel-key"))
            .respond_with(template)
            .mount(&self.server)
            .await;
    }

    async fn delivery_reads(&self) -> usize {
        self.server
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .filter(|r| r.url.path() == "/external/deliveries/")
            .count()
    }

    fn at(&self, ms: i64) {
        self.clock.set(ms);
    }

    fn store(&self) -> &Store {
        &self.store.store
    }
}

fn completed_only() -> Value {
    json!({"success": true, "deliveries": [{
        "tracking_number": "DONE1", "carrier_code": "uniuni", "description": "Done",
        "status_code": 0, "events": [],
    }]})
}

async fn read_state(store: &Store) -> ReadState {
    state::load(store).await.unwrap().1.unwrap()
}

#[test]
fn decodes_the_recorded_response() {
    let deliveries = decode_deliveries(FIXTURE.as_bytes()).unwrap();
    assert_eq!(deliveries.len(), 4);
    let codes: Vec<i64> = deliveries.iter().map(|d| d.status_code).collect();
    assert_eq!(codes, [2, 8, 8, 4]);
    assert_eq!(deliveries[0].date_expected, None);
    assert_eq!(deliveries[0].events.len(), 16);
    assert_eq!(deliveries[0].events[0].location, None);
    assert_eq!(
        deliveries[1].date_expected.as_deref(),
        Some("2026-10-09 00:00:00")
    );
    assert_eq!(deliveries[1].events[0].date.as_deref(), Some("--//--"));
    assert_eq!(
        deliveries[3].events[0].location.as_deref(),
        Some("Springfield ST")
    );
}

#[test]
fn decodes_unknown_status_codes_and_missing_fields() {
    let body = json!({"success": true, "deliveries": [
        {"tracking_number": "X1", "status_code": 42},
    ]});
    let deliveries = decode_deliveries(body.to_string().as_bytes()).unwrap();
    assert_eq!(deliveries[0].carrier_code, None);
    assert!(deliveries[0].events.is_empty());
    assert_eq!(
        ParcelDeliveryStatus::from_code(deliveries[0].status_code),
        ParcelDeliveryStatus::Unknown
    );
}

#[test]
fn reports_an_unsuccessful_body_and_malformed_json() {
    let body = json!({"success": false, "error_message": "Invalid API key"});
    assert!(matches!(
        decode_deliveries(body.to_string().as_bytes()),
        Err(DeliveriesError::Unsuccessful(message)) if message == "Invalid API key"
    ));
    assert!(matches!(
        decode_deliveries(b"<html>"),
        Err(DeliveriesError::Decode(_))
    ));
}

#[tokio::test]
async fn client_sends_the_key_and_maps_429_and_oversize_bodies() {
    let server = MockServer::start().await;
    let http = omni_testkit::mock_http(&server, &["https://api.parcel.app"]);
    let client = DeliveriesClient::new(http, "parcel-key".to_owned()).unwrap();

    Mock::given(method("GET"))
        .and(path("/external/deliveries/"))
        .and(header("api-key", "parcel-key"))
        .respond_with(ResponseTemplate::new(200).set_body_string(FIXTURE))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    assert_eq!(client.fetch().await.unwrap().len(), 4);

    server.reset().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "7200"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    assert!(matches!(
        client.fetch().await,
        Err(DeliveriesError::RateLimited {
            retry_after_ms: Some(7_200_000)
        })
    ));

    server.reset().await;
    let huge = format!(
        "{{\"success\":true,\"deliveries\":[],\"pad\":\"{}\"}}",
        "x".repeat(omni_parcel::deliveries::api::RESPONSE_LIMIT)
    );
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(huge))
        .mount(&server)
        .await;
    assert!(matches!(
        client.fetch().await,
        Err(DeliveriesError::Http(omni_http::HttpError::TooLarge { .. }))
    ));
}

#[tokio::test]
async fn reads_once_then_waits_30_minutes_while_deliveries_are_active() {
    let h = Harness::new(SideEffectMode::Live).await;
    h.respond(ResponseTemplate::new(200).set_body_string(FIXTURE))
        .await;

    assert_eq!(
        h.task.tick().await.unwrap(),
        TickOutcome::Read {
            deliveries: 4,
            active: 4
        }
    );
    assert_eq!(h.delivery_reads().await, 1);

    // Restarts and manual runs inside the interval do not read.
    for minutes in [0, 10, 20] {
        h.at(NOW + minutes * MINUTE);
        assert!(matches!(
            h.task.tick().await.unwrap(),
            TickOutcome::Skipped {
                reason: SkipReason::NotDue,
                ..
            }
        ));
    }
    assert_eq!(h.delivery_reads().await, 1);

    h.at(NOW + 30 * MINUTE);
    assert!(matches!(
        h.task.tick().await.unwrap(),
        TickOutcome::Read { .. }
    ));
    assert_eq!(h.delivery_reads().await, 2);

    let snapshot = state::load(h.store()).await.unwrap().0.unwrap();
    let uniuni = &snapshot.deliveries[3];
    assert_eq!(uniuni.carrier_name.as_deref(), Some("UniUni"));
    assert_eq!(uniuni.events[0].description, "Out for delivery");
}

#[tokio::test]
async fn reads_every_3_hours_when_nothing_is_active_unless_omni_submits_a_number() {
    let h = Harness::new(SideEffectMode::Live).await;
    h.respond(ResponseTemplate::new(200).set_body_json(completed_only()))
        .await;
    assert!(matches!(
        h.task.tick().await.unwrap(),
        TickOutcome::Read { active: 0, .. }
    ));

    h.at(NOW + 2 * HOUR);
    assert!(matches!(
        h.task.tick().await.unwrap(),
        TickOutcome::Skipped {
            reason: SkipReason::NotDue,
            ..
        }
    ));
    assert_eq!(h.delivery_reads().await, 1);

    // A tracking number submitted after the read brings the 30-minute cadence back.
    persistence::record(
        h.store(),
        DeliveryAttempt {
            tracking_number: "NEW1".to_owned(),
            carrier_code: "uniuni".to_owned(),
            description: "New".to_owned(),
            submitted_at: NOW + 2 * HOUR,
            email_id: "m1".to_owned(),
        },
        SubmissionStatus::Submitted,
        Some(1),
    )
    .await
    .unwrap();
    assert!(matches!(
        h.task.tick().await.unwrap(),
        TickOutcome::Read { .. }
    ));
    assert_eq!(h.delivery_reads().await, 2);

    h.at(NOW + 5 * HOUR);
    assert!(matches!(
        h.task.tick().await.unwrap(),
        TickOutcome::Read { .. }
    ));
    assert_eq!(h.delivery_reads().await, 3);
}

#[tokio::test]
async fn a_429_backs_off_for_an_hour_and_counts_against_the_budget() {
    let h = Harness::new(SideEffectMode::Live).await;
    h.respond(ResponseTemplate::new(429)).await;
    let outcome = h.task.tick().await.unwrap();
    let TickOutcome::RateLimited { until } = outcome else {
        panic!("expected a back-off, got {outcome:?}");
    };
    assert!(until >= NOW + HOUR);

    h.at(NOW + 45 * MINUTE);
    assert!(matches!(
        h.task.tick().await.unwrap(),
        TickOutcome::Skipped {
            reason: SkipReason::BackingOff,
            ..
        }
    ));
    assert_eq!(h.delivery_reads().await, 1);
    let read = read_state(h.store()).await;
    assert_eq!(read.attempts.len(), 1);
    assert_eq!(read.last_status, Some(429));

    h.respond(ResponseTemplate::new(200).set_body_string(FIXTURE))
        .await;
    h.at(until);
    assert!(matches!(
        h.task.tick().await.unwrap(),
        TickOutcome::Read { .. }
    ));
    let read = read_state(h.store()).await;
    assert_eq!(read.backoff_until, None);
    assert_eq!(read.last_error, None);
}

#[tokio::test]
async fn an_auth_failure_is_an_error_and_still_spends_a_read() {
    let h = Harness::new(SideEffectMode::Live).await;
    h.respond(ResponseTemplate::new(401).set_body_string("{\"error\":\"bad key\"}"))
        .await;
    assert!(h.task.tick().await.is_err());
    let read = read_state(h.store()).await;
    assert_eq!(read.attempts.len(), 1);
    assert!(read.attempts[0] >= NOW && read.attempts[0] < NOW + MINUTE);
    assert_eq!(read.last_status, Some(401));

    h.at(NOW + 10 * MINUTE);
    assert!(matches!(
        h.task.tick().await.unwrap(),
        TickOutcome::Skipped { .. }
    ));
    assert_eq!(h.delivery_reads().await, 1);
}

#[tokio::test]
async fn transient_failures_wait_for_the_next_due_read() {
    let h = Harness::new(SideEffectMode::Live).await;
    h.respond(ResponseTemplate::new(503)).await;
    assert!(matches!(
        h.task.tick().await.unwrap(),
        TickOutcome::Failed { .. }
    ));
    h.at(NOW + 10 * MINUTE);
    assert!(matches!(
        h.task.tick().await.unwrap(),
        TickOutcome::Skipped { .. }
    ));
    assert_eq!(h.delivery_reads().await, 1);
}

fn run_context() -> omni_tasks::RunContext {
    omni_tasks::RunContext {
        run_id: "run-1".to_owned(),
        task_name: "ParcelDeliveries".to_owned(),
        trigger: omni_tasks::Trigger::Schedule,
        scheduled_for: None,
        cancel: tokio_util::sync::CancellationToken::new(),
    }
}

#[tokio::test]
async fn failed_and_rate_limited_runs_report_degraded_and_skips_do_not() {
    use omni_tasks::Task as _;
    let h = Harness::new(SideEffectMode::Live).await;
    let cx = run_context();
    h.respond(ResponseTemplate::new(503)).await;
    let (result, degraded) = omni_tasks::collect_degraded(h.task.run(&cx)).await;
    result.unwrap();
    assert_eq!(degraded.len(), 1);
    assert!(degraded[0].starts_with("Read failed: "), "{degraded:?}");

    h.at(NOW + 10 * MINUTE);
    let (result, degraded) = omni_tasks::collect_degraded(h.task.run(&cx)).await;
    result.unwrap();
    assert!(degraded.is_empty(), "{degraded:?}");

    h.respond(ResponseTemplate::new(429)).await;
    h.at(NOW + 30 * MINUTE);
    let (result, degraded) = omni_tasks::collect_degraded(h.task.run(&cx)).await;
    result.unwrap();
    assert_eq!(degraded.len(), 1);
    assert!(degraded[0].starts_with("Rate limited; "), "{degraded:?}");
    // `respond` resets the recorded requests: only the 429 read remains.
    assert_eq!(h.delivery_reads().await, 1);
}

#[tokio::test]
async fn never_reads_while_side_effects_are_recorded() {
    let h = Harness::new(SideEffectMode::Record).await;
    h.respond(ResponseTemplate::new(200).set_body_string(FIXTURE))
        .await;
    assert_eq!(h.task.tick().await.unwrap(), TickOutcome::Disabled);
    assert_eq!(h.delivery_reads().await, 0);
    assert!(state::load(h.store()).await.unwrap().1.is_none());
}

async fn seeded_store(clock: &Arc<TestClock>) -> TestStore {
    let store = TestStore::new(clock.clone()).await;
    let raw = decode_deliveries(FIXTURE.as_bytes()).unwrap();
    let mut raw = raw;
    raw.push(
        decode_deliveries(completed_only().to_string().as_bytes())
            .unwrap()
            .remove(0),
    );
    let cached = state::to_cached(raw, |code| (code == "uniuni").then(|| "UniUni".to_owned()));
    state::reserve(&store.store, NOW).await.unwrap();
    state::record_success(&store.store, NOW, cached)
        .await
        .unwrap();
    persistence::record(
        &store.store,
        DeliveryAttempt {
            tracking_number: "uus0000000000000001".to_owned(),
            carrier_code: "uniuni".to_owned(),
            description: "Parts".to_owned(),
            submitted_at: NOW - HOUR,
            email_id: "<m1@example.com>".to_owned(),
        },
        SubmissionStatus::Submitted,
        Some(1),
    )
    .await
    .unwrap();
    store
}

#[tokio::test]
async fn the_route_serves_the_cache_with_active_deliveries_first_and_sources() {
    let clock = TestClock::new(NOW);
    let store = seeded_store(&clock).await;
    let response = omni_parcel::deliveries::load_response(&store.store, true)
        .await
        .unwrap();
    assert_eq!(response.fetched_at, Some(NOW));
    assert_eq!(response.active_count, 4);
    assert_eq!(response.next_read_after, Some(NOW + 28 * MINUTE));
    assert_eq!(response.deliveries.len(), 5);
    assert!(!response.deliveries[4].active);
    let uniuni = &response.deliveries[3];
    assert_eq!(uniuni.status, ParcelDeliveryStatus::OutForDelivery);
    let source = uniuni.source.as_ref().unwrap();
    assert_eq!(source.activity_id, "ParcelTracker#<m1@example.com>");

    let app = omni_testkit::TestApp::new().await;
    let router = omni_parcel::deliveries::router(store.store.clone(), true);
    let (status, body) = app.get_json(&router, "/api/parcels").await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let decoded: ParcelsResponse = serde_json::from_value(body).unwrap();
    assert_eq!(decoded, response);
}

#[tokio::test]
async fn the_route_reports_an_unconfigured_empty_cache() {
    let app = omni_testkit::TestApp::new().await;
    let subsystem = omni_parcel::subsystem(
        &app.ctx,
        omni_email::triage::EmailTriage::with_model(
            app.ctx.ai.clone(),
            app.ctx.config.clone(),
            app.ctx.store.clone(),
        ),
    )
    .unwrap();
    assert!(subsystem.tasks.is_empty());
    let (status, body) = app.get_json(&subsystem.router, "/api/parcels").await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(
        body,
        json!({
            "configured": false, "fetchedAt": null, "lastAttemptAt": null,
            "nextReadAfter": null, "backoffUntil": null, "lastError": null,
            "activeCount": 0, "deliveries": [],
        })
    );
}

async fn call(tools: &[McpTool], name: &str, input: Value) -> Result<Value, String> {
    let tool = tools.iter().find(|t| t.meta.name == name).unwrap();
    match tool.handler.call(input, standalone_context("test")).await {
        Ok(ToolOutput::Structured(map)) => Ok(Value::Object(map)),
        Ok(ToolOutput::Custom { structured, .. }) => Ok(Value::Object(structured)),
        Err(e) => Err(e.message),
    }
}

#[tokio::test]
async fn parcels_list_filters_and_bounds_the_cache() {
    let clock = TestClock::new(NOW);
    let store = seeded_store(&clock).await;
    let tools = omni_parcel::mcp::tools(store.store.clone(), true).unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t.meta.name.as_str()).collect();
    assert_eq!(names, ["parcels_list", "parcels_get"]);

    let out = call(&tools, "parcels_list", json!({})).await.unwrap();
    assert_eq!(out["total"], 4);
    assert_eq!(out["activeCount"], 4);
    assert_eq!(out["cache"]["configured"], true);
    assert_eq!(out["cache"]["fetchedAt"], "2026-09-21T14:13:20.000Z");
    assert_eq!(out["deliveries"][0]["events"].as_array().unwrap().len(), 3);
    assert_eq!(out["deliveries"][0]["eventCount"], 16);
    assert_eq!(out["deliveries"][3]["statusLabel"], "Out for delivery");
    assert_eq!(
        out["deliveries"][3]["source"]["activityId"],
        "ParcelTracker#<m1@example.com>"
    );

    let out = call(
        &tools,
        "parcels_list",
        json!({"filter": "all", "limit": 1, "events": 0}),
    )
    .await
    .unwrap();
    assert_eq!(out["total"], 5);
    assert_eq!(out["deliveries"].as_array().unwrap().len(), 1);
    assert_eq!(out["deliveries"][0]["events"], json!([]));

    assert!(
        call(&tools, "parcels_list", json!({"limit": 51}))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn parcels_get_matches_loosely_and_reports_submissions_missing_from_the_cache() {
    let clock = TestClock::new(NOW);
    let store = seeded_store(&clock).await;
    persistence::record(
        &store.store,
        DeliveryAttempt {
            tracking_number: "LATE1".to_owned(),
            carrier_code: "ups".to_owned(),
            description: "Late".to_owned(),
            submitted_at: NOW + MINUTE,
            email_id: "m2".to_owned(),
        },
        SubmissionStatus::Submitted,
        Some(1),
    )
    .await
    .unwrap();
    let tools = omni_parcel::mcp::tools(store.store.clone(), true).unwrap();

    let out = call(
        &tools,
        "parcels_get",
        json!({"trackingNumber": "uus 0000000000000001"}),
    )
    .await
    .unwrap();
    assert_eq!(out["delivery"]["status"], "out_for_delivery");
    assert_eq!(out["delivery"]["events"].as_array().unwrap().len(), 4);
    assert_eq!(out["submitted"]["emailId"], "<m1@example.com>");

    let out = call(&tools, "parcels_get", json!({"trackingNumber": "late1"}))
        .await
        .unwrap();
    assert_eq!(out["delivery"], Value::Null);
    assert_eq!(out["submitted"]["emailId"], "m2");
}
