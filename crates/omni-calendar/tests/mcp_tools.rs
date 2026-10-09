//! Calendar MCP tools: metadata, input refinements, and the create/update/delete flows against a
//! mock CalDAV server.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{CALENDAR_URL, caldav};
use omni_calendar::mcp::{CalendarTools, calendar_tools};
use omni_calendar::persistence::{self, compute_event_hash, compute_mcp_event_uid};
use omni_mcp_kit::registry::standalone_context;
use omni_mcp_kit::{McpTool, ToolOutput, ToolPhase};
use omni_testkit::TestApp;
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const NAMES: [&str; 7] = [
    "calendar_events_list",
    "calendar_event_get",
    "calendar_status",
    "calendar_event_preview",
    "calendar_event_create",
    "calendar_event_update",
    "calendar_event_delete",
];

fn tools(app: &TestApp, server: &MockServer) -> Vec<McpTool> {
    calendar_tools(CalendarTools {
        store: app.ctx.store.clone(),
        caldav: caldav(app, server, Some(CALENDAR_URL)),
        clock: app.ctx.clock.clone(),
    })
    .unwrap()
}

async fn call(tools: &[McpTool], name: &str, input: Value) -> Result<Value, (ToolPhase, String)> {
    let tool = tools.iter().find(|t| t.meta.name == name).unwrap();
    match tool.handler.call(input, standalone_context("test")).await {
        Ok(ToolOutput::Structured(map)) => Ok(Value::Object(map)),
        Ok(ToolOutput::Custom { structured, .. }) => Ok(Value::Object(structured)),
        Err(e) => Err((e.phase, e.message)),
    }
}

fn dentist() -> Value {
    json!({ "title": "  🦷 Dentist  ", "startDate": "2026-09-01", "startTime": "09:00", "allDay": false, "location": "Clinic", "reminderMinutes": 60 })
}

#[tokio::test]
async fn registers_the_seven_tools_in_serving_order() {
    let app = TestApp::new().await;
    let server = MockServer::start().await;
    let names: Vec<&str> = tools(&app, &server)
        .iter()
        .map(|t| t.meta.name.as_str())
        .collect();
    assert_eq!(names, NAMES);
}

#[tokio::test]
async fn status_reports_provider_and_counts_without_contacting_caldav() {
    let app = TestApp::new().await;
    let server = MockServer::start().await;
    let out = call(&tools(&app, &server), "calendar_status", json!({}))
        .await
        .unwrap();
    assert_eq!(
        out,
        json!({ "configured": true, "provider": "icloud", "tracked": { "active": 0, "cancelled": 0, "total": 0 } })
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn preview_renders_ics_without_state_changes() {
    let app = TestApp::new().await;
    let server = MockServer::start().await;
    let out = call(
        &tools(&app, &server),
        "calendar_event_preview",
        json!({ "event": dentist() }),
    )
    .await
    .unwrap();
    assert_eq!(out["eventHash"], "dentist|2026-09-01|09:00");
    assert_eq!(out["duplicateTrackedEvent"], false);
    let ics = out["iCalendar"].as_str().unwrap();
    assert!(ics.contains("UID:preview@omni-notify\r\n"));
    assert!(ics.contains("SUMMARY:🦷 Dentist\r\n"));
    assert!(ics.contains("TRIGGER:-PT1H\r\n"));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn rejects_inputs_the_schema_refinements_reject() {
    let app = TestApp::new().await;
    let server = MockServer::start().await;
    let t = tools(&app, &server);
    for (event, needle) in [
        (
            json!({ "title": "X", "startDate": "2026-02-30", "allDay": true }),
            "Invalid date",
        ),
        (
            json!({ "title": "X", "startDate": "2026-09-01", "allDay": false }),
            "Timed events require startTime",
        ),
        (
            json!({ "title": "X", "startDate": "2026-09-01", "startTime": "09:00", "allDay": true }),
            "All-day events cannot include times",
        ),
        (
            json!({ "title": "X", "startDate": "2026-09-01", "startTime": "09:00", "endTime": "10:00", "duration": "PT1H", "allDay": false }),
            "either endTime or duration",
        ),
        (
            json!({ "title": "X", "startDate": "2026-09-02", "endDate": "2026-09-01", "allDay": true }),
            "endDate cannot precede startDate",
        ),
        (
            json!({ "title": "X", "startDate": "2026-09-01", "allDay": true, "timeZone": "Mars/Base" }),
            "valid IANA time zone",
        ),
        (
            json!({ "title": "   ", "startDate": "2026-09-01", "allDay": true }),
            "title",
        ),
    ] {
        let (phase, message) = call(&t, "calendar_event_create", json!({ "event": event }))
            .await
            .unwrap_err();
        assert_eq!(phase, ToolPhase::Input);
        assert!(message.contains(needle), "{needle}: {message}");
    }
    // JSON Schema rejects unknown fields and malformed patterns.
    let (phase, _) = call(
        &t,
        "calendar_event_create",
        json!({ "event": { "title": "X", "startDate": "2026-09-01", "allDay": true, "extra": 1 } }),
    )
    .await
    .unwrap_err();
    assert_eq!(phase, ToolPhase::Input);
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn create_update_delete_round_trip() {
    let app = TestApp::new().await;
    let server = MockServer::start().await;
    let hash = compute_event_hash("🦷 Dentist", "2026-09-01", Some("09:00"));
    let uid = compute_mcp_event_uid(&hash);
    let event_path = format!("/123/calendars/home/{uid}.ics");
    Mock::given(method("PUT"))
        .and(path(event_path.as_str()))
        .and(header("If-None-Match", "*"))
        .respond_with(ResponseTemplate::new(201))
        .expect(1)
        .mount(&server)
        .await;
    let t = tools(&app, &server);

    let created = call(&t, "calendar_event_create", json!({ "event": dentist() }))
        .await
        .unwrap();
    assert_eq!(created["status"], "created");
    assert_eq!(created["event"]["calendarEventId"], uid.as_str());
    assert_eq!(created["event"]["sourceEmailId"], "mcp");
    assert_eq!(created["event"]["title"], "🦷 Dentist");
    assert_eq!(created["event"]["reminderMinutes"], json!(60));
    assert_eq!(created["event"]["endDate"], Value::Null);
    assert_eq!(created["event"]["status"], "active");

    // A repeat is a local no-op.
    let again = call(&t, "calendar_event_create", json!({ "event": dentist() }))
        .await
        .unwrap();
    assert_eq!(again["status"], "already_exists");

    let listed = call(&t, "calendar_events_list", json!({ "query": "clinic" }))
        .await
        .unwrap();
    assert_eq!(listed["total"], 1);
    assert_eq!(listed["nextCursor"], Value::Null);
    let got = call(&t, "calendar_event_get", json!({ "eventHash": hash }))
        .await
        .unwrap();
    assert_eq!(got["event"]["eventHash"], hash.as_str());

    // Unchanged patch makes no remote write.
    let unchanged = call(
        &t,
        "calendar_event_update",
        json!({ "eventHash": hash, "changes": { "title": "Dentist!" } }),
    )
    .await
    .unwrap();
    assert_eq!(unchanged["status"], "unchanged");

    Mock::given(method("PUT"))
        .and(path(event_path.as_str()))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    let updated = call(
        &t,
        "calendar_event_update",
        json!({ "eventHash": hash, "changes": { "startTime": "10:30", "location": null } }),
    )
    .await
    .unwrap();
    assert_eq!(updated["status"], "updated");
    assert_eq!(updated["event"]["eventHash"], "dentist|2026-09-01|10:30");
    assert_eq!(updated["event"]["location"], Value::Null);
    assert_eq!(updated["event"]["calendarEventId"], uid.as_str());
    let old = persistence::get_tracked_event(&app.ctx.store, &hash)
        .await
        .unwrap()
        .unwrap();
    assert!(old.is_cancelled());
    let (_, message) = call(
        &t,
        "calendar_event_update",
        json!({ "eventHash": hash, "changes": { "title": "Again" } }),
    )
    .await
    .unwrap_err();
    assert_eq!(message, "Cancelled events cannot be updated");

    Mock::given(method("DELETE"))
        .and(path(event_path.as_str()))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    let deleted = call(
        &t,
        "calendar_event_delete",
        json!({ "eventHash": "dentist|2026-09-01|10:30" }),
    )
    .await
    .unwrap();
    assert_eq!(deleted["status"], "deleted");
    assert_eq!(deleted["event"]["status"], "cancelled");
    let again = call(
        &t,
        "calendar_event_delete",
        json!({ "eventHash": "dentist|2026-09-01|10:30" }),
    )
    .await
    .unwrap();
    assert_eq!(again["status"], "already_deleted");

    let status = call(&t, "calendar_status", json!({})).await.unwrap();
    assert_eq!(
        status["tracked"],
        json!({ "active": 0, "cancelled": 2, "total": 2 })
    );
    let all = call(&t, "calendar_events_list", json!({ "status": "all" }))
        .await
        .unwrap();
    let starts: Vec<&str> = all["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["startTime"].as_str().unwrap())
        .collect();
    assert_eq!(starts, ["09:00", "10:30"]);
}

#[tokio::test]
async fn unknown_events_and_discovery_failures_surface_the_innermost_cause() {
    let app = TestApp::new().await;
    let server = MockServer::start().await;
    let t = tools(&app, &server);
    let (phase, message) = call(&t, "calendar_event_get", json!({ "eventHash": "nope" }))
        .await
        .unwrap_err();
    assert_eq!(phase, ToolPhase::Execute);
    assert_eq!(message, "Unknown tracked calendar event: nope");

    let unconfigured = calendar_tools(CalendarTools {
        store: app.ctx.store.clone(),
        caldav: omni_calendar::caldav(&app.ctx),
        clock: app.ctx.clock.clone(),
    })
    .unwrap();
    let (_, message) = call(
        &unconfigured,
        "calendar_event_create",
        json!({ "event": dentist() }),
    )
    .await
    .unwrap_err();
    assert_eq!(message, "No CalDAV provider configured");
    let status = call(&unconfigured, "calendar_status", json!({}))
        .await
        .unwrap();
    assert_eq!(status["configured"], false);
    assert_eq!(status["provider"], Value::Null);
}

#[tokio::test]
async fn create_reconciles_a_remote_event_that_already_exists() {
    let app = TestApp::new().await;
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(412))
        .mount(&server)
        .await;
    let created = call(
        &tools(&app, &server),
        "calendar_event_create",
        json!({ "event": dentist() }),
    )
    .await
    .unwrap();
    assert_eq!(created["status"], "reconciled");
}

#[tokio::test]
async fn legacy_rows_without_all_day_list_as_timed_and_need_all_day_to_update() {
    let app = TestApp::new().await;
    let server = MockServer::start().await;
    let mut legacy = persistence::CreatedCalendarEvent::from_event(
        "cheese contest|2026-04-01|allday".to_owned(),
        "e".to_owned(),
        "omni-x@omni-notify".to_owned(),
        &omni_calendar::extraction::schema::ExtractedEvent::new(
            omni_calendar::extraction::schema::EventAction::Create,
            "🧀 Cheese Contest",
            "2026-04-01",
            true,
        ),
        1,
    );
    legacy.all_day = None;
    persistence::record_created_event(&app.ctx.store, legacy)
        .await
        .unwrap();
    let t = tools(&app, &server);
    let listed = call(&t, "calendar_events_list", json!({})).await.unwrap();
    assert_eq!(listed["items"][0]["allDay"], false);
    let (phase, message) = call(
        &t,
        "calendar_event_update",
        json!({ "eventHash": "cheese contest|2026-04-01|allday", "changes": { "title": "Cheese" } }),
    )
    .await
    .unwrap_err();
    // The merged event is re-validated during execution.
    assert_eq!(phase, ToolPhase::Execute);
    assert_eq!(message, "allDay: Required");
}

#[tokio::test]
async fn update_reports_an_invalid_merged_event_as_an_execute_error() {
    let app = TestApp::new().await;
    let server = MockServer::start().await;
    let mut timed = omni_calendar::extraction::schema::ExtractedEvent::new(
        omni_calendar::extraction::schema::EventAction::Create,
        "Dentist",
        "2026-09-01",
        false,
    );
    timed.start_time = Some("09:00".to_owned());
    persistence::record_created_event(
        &app.ctx.store,
        persistence::CreatedCalendarEvent::from_event(
            "dentist|2026-09-01|09:00".to_owned(),
            "e".to_owned(),
            "omni-d@omni-notify".to_owned(),
            &timed,
            1,
        ),
    )
    .await
    .unwrap();
    let t = tools(&app, &server);
    let (phase, message) = call(
        &t,
        "calendar_event_update",
        json!({ "eventHash": "dentist|2026-09-01|09:00", "changes": { "allDay": true } }),
    )
    .await
    .unwrap_err();
    assert_eq!(phase, ToolPhase::Execute);
    assert!(
        message.contains("All-day events cannot include times"),
        "{message}"
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn list_rejects_a_blank_query() {
    let app = TestApp::new().await;
    let server = MockServer::start().await;
    let t = tools(&app, &server);
    let (phase, _) = call(&t, "calendar_events_list", json!({ "query": "   " }))
        .await
        .unwrap_err();
    assert_eq!(phase, ToolPhase::Input);
}
