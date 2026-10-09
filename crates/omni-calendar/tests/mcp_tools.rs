//! Calendar MCP tools without a calendar: input refinements, the
//! unconfigured state, and the email-created event listing.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_calendar::extraction::schema::{EventAction, ExtractedEvent};
use omni_calendar::mcp::{CalendarTools, calendar_tools};
use omni_calendar::persistence::{self, CreatedCalendarEvent};
use omni_calendar::primary::{PrimaryCalendar, PrimaryDeps};
use omni_http::SideEffectMode;
use omni_mcp_kit::registry::standalone_context;
use omni_mcp_kit::{McpTool, ToolOutput, ToolPhase};
use omni_testkit::TestApp;
use serde_json::{Value, json};

fn tools(app: &TestApp) -> Vec<McpTool> {
    let primary = PrimaryCalendar::new(PrimaryDeps {
        http: omni_testkit::no_network(),
        settings: None,
        store: app.ctx.store.clone(),
        clock: app.ctx.clock.clone(),
        mode: SideEffectMode::Record,
        recorded: Default::default(),
        default_tz: "America/Vancouver".to_owned(),
        tracker: None,
        ports: app.ctx.ports.clone(),
    });
    calendar_tools(CalendarTools {
        store: app.ctx.store.clone(),
        primary,
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

#[tokio::test]
async fn reports_the_missing_provider_without_network() {
    let app = TestApp::new().await;
    let t = tools(&app);
    let status = call(&t, "calendar_status", json!({})).await.unwrap();
    assert_eq!(status["configured"], false);
    assert_eq!(status["state"], "not_configured");
    assert_eq!(
        status["tracked"],
        json!({ "active": 0, "cancelled": 0, "total": 0 })
    );
    let (phase, message) = call(
        &t,
        "calendar_event_create",
        json!({ "idempotencyKey": "abcdefghijklmnop", "title": "X", "start": "2026-10-01" }),
    )
    .await
    .unwrap_err();
    assert_eq!(phase, ToolPhase::Execute);
    assert!(message.starts_with("[not_configured]"), "{message}");
}

#[tokio::test]
async fn rejects_inputs_the_schema_cannot() {
    let app = TestApp::new().await;
    let t = tools(&app);
    let cases = [
        (
            "calendar_event_create",
            json!({ "idempotencyKey": "abcdefghijklmnop", "title": "X", "start": "2026-02-30" }),
            "not a real date",
        ),
        (
            "calendar_event_create",
            json!({ "idempotencyKey": "abcdefghijklmnop", "title": "  ", "start": "2026-10-01" }),
            "title: must not be blank",
        ),
        (
            "calendar_event_create",
            json!({ "idempotencyKey": "abcdefghijklmnop", "title": "X", "start": "2026-10-01T09:00", "timeZone": "Mars/Base" }),
            "unknown time zone",
        ),
        (
            "calendar_event_create",
            json!({ "idempotencyKey": "abcdefghijklmnop", "title": "X", "start": "2026-10-01T09:00", "end": "2026-10-01T10:00", "durationMinutes": 30 }),
            "not both",
        ),
        (
            "calendar_event_create",
            json!({ "idempotencyKey": "abcdefghijklmnop", "title": "X", "start": "2026-10-01", "recurrence": { "frequency": "weekly", "count": 3, "untilDate": "2026-12-01" } }),
            "count or untilDate",
        ),
        (
            "calendar_event_create",
            json!({ "idempotencyKey": "abcdefghijklmnop", "title": "X", "start": "2026-10-01", "alarms": [{}] }),
            "exactly one of minutesBefore or at",
        ),
        (
            "calendar_event_update",
            json!({ "idempotencyKey": "abcdefghijklmnop", "eventId": "a.ics", "scope": "occurrence", "changes": { "title": "Y" } }),
            "recurrenceId is required",
        ),
        (
            "calendar_event_update",
            json!({ "idempotencyKey": "abcdefghijklmnop", "eventId": "a.ics", "changes": { "end": "2026-10-01T10:00" } }),
            "need start",
        ),
        (
            "calendar_event_preview",
            json!({}),
            "exactly one of create, update or delete",
        ),
        (
            "calendar_event_get",
            json!({ "eventId": "../x.ics" }),
            "eventId must be a resource name",
        ),
        (
            "calendar_events_list",
            json!({ "from": "2026-01-01", "to": "2027-06-01" }),
            "at most 366 days",
        ),
    ];
    for (tool, input, expected) in cases {
        let (phase, message) = call(&t, tool, input.clone()).await.unwrap_err();
        assert_eq!(phase, ToolPhase::Input, "{tool} {input}: {message}");
        assert!(message.contains(expected), "{tool} {input}: {message}");
    }
}

#[tokio::test]
async fn lists_email_created_events_with_filters() {
    let app = TestApp::new().await;
    for (hash, title, date, cancelled) in [
        ("a", "🦷 Dentist", "2026-09-01", false),
        ("b", "Haircut", "2026-09-05", false),
        ("c", "Old thing", "2026-08-01", true),
    ] {
        let mut row = CreatedCalendarEvent::from_event(
            hash.to_owned(),
            "email-1".to_owned(),
            format!("omni-{hash}@omni-notify"),
            &ExtractedEvent::new(EventAction::Create, title, date, true),
            1,
        );
        if cancelled {
            row.status = Some(persistence::EventStatus::Cancelled);
        }
        persistence::record_created_event(&app.ctx.store, row)
            .await
            .unwrap();
    }
    let t = tools(&app);
    let out = call(&t, "calendar_tracked_events_list", json!({}))
        .await
        .unwrap();
    assert_eq!(out["total"], 2);
    assert_eq!(out["items"][0]["title"], "🦷 Dentist");
    assert_eq!(out["items"][0]["eventId"], "omni-a@omni-notify.ics");
    let out = call(
        &t,
        "calendar_tracked_events_list",
        json!({ "status": "all", "query": "old", "limit": 1 }),
    )
    .await
    .unwrap();
    assert_eq!(out["total"], 1);
    assert_eq!(out["items"][0]["status"], "cancelled");
    let out = call(
        &t,
        "calendar_tracked_events_list",
        json!({ "from": "2026-09-02", "through": "2026-09-30" }),
    )
    .await
    .unwrap();
    assert_eq!(out["items"][0]["title"], "Haircut");
}
