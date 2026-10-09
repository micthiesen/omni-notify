//! CalDAV writes, plus the cross-calendar move (403) fallback and record mode.
//!
//! The interruption cases assert that aborting the task drops the in-flight
//! request. Update and delete requests are bounded by the shared 15 s request
//! timeout, asserted in `http.rs`; these cases check the outcomes.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use common::{CALENDAR_URL, icloud_http, session};
use omni_calendar::caldav::api::MERGE_ATTEMPTS;
use omni_calendar::caldav::http::CALDAV_ERROR_MAX_BYTES;
use omni_calendar::caldav::ics::build_icalendar;
use omni_calendar::caldav::{CaldavWriter, CreateOutcome, DeleteOutcome, UpdateOutcome};
use omni_calendar::extraction::schema::{EventAction, ExtractedEvent};
use omni_http::SideEffectMode;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const UID: &str = "stable@omni-notify";
const EVENT_PATH: &str = "/123/calendars/home/stable@omni-notify.ics";

fn event() -> ExtractedEvent {
    let mut event = ExtractedEvent::new(EventAction::Create, "Dentist", "2026-09-01", false);
    event.start_time = Some("09:00".to_owned());
    event
}

fn writer(server: &MockServer) -> CaldavWriter {
    CaldavWriter::new(
        icloud_http(server),
        omni_testkit::test_clock(omni_testkit::TEST_EPOCH_MS),
        SideEffectMode::Live,
        "America/Vancouver",
    )
}

async fn wait_for_requests(server: &MockServer, n: usize) {
    for _ in 0..200 {
        if server.received_requests().await.unwrap_or_default().len() >= n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("server never received {n} request(s)");
}

#[tokio::test]
async fn aborts_an_in_flight_put_when_dropped() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(201).set_delay(Duration::from_secs(60)))
        .mount(&server)
        .await;
    let w = writer(&server);
    let task = tokio::spawn(async move { w.create(&session(), &event(), UID).await });
    wait_for_requests(&server, 1).await;
    assert!(!task.is_finished());
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests[0].method.as_str(), "PUT");
}

#[tokio::test]
async fn aborts_an_in_flight_delete_when_dropped() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .respond_with(ResponseTemplate::new(204).set_delay(Duration::from_secs(60)))
        .mount(&server)
        .await;
    let w = writer(&server);
    let task = tokio::spawn(async move { w.delete(&session(), UID).await });
    wait_for_requests(&server, 1).await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests[0].method.as_str(), "DELETE");
}

#[tokio::test]
async fn reconciles_a_repeated_deterministic_create_after_an_ambiguous_response() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path(EVENT_PATH))
        .and(header("If-None-Match", "*"))
        .and(header("Authorization", "Basic secret"))
        .respond_with(ResponseTemplate::new(412))
        .expect(1)
        .mount(&server)
        .await;
    let result = writer(&server)
        .create(&session(), &event(), UID)
        .await
        .unwrap();
    assert_eq!(
        result,
        CreateOutcome::AlreadyExists {
            event_uid: UID.to_owned()
        }
    );
    let request = &server.received_requests().await.unwrap()[0];
    assert_eq!(
        request.headers.get("content-type").unwrap(),
        "text/calendar; charset=utf-8"
    );
    assert_eq!(
        format!("{CALENDAR_URL}{UID}.ics"),
        "https://p42-caldav.icloud.com/123/calendars/home/stable@omni-notify.ics"
    );
}

fn moved() -> ExtractedEvent {
    let mut event = event();
    event.start_time = Some("10:00".to_owned());
    event
}

/// The server copy: what the pipeline wrote plus a manual edit.
fn server_copy(location: &str) -> String {
    build_icalendar(
        &event(),
        UID,
        omni_testkit::TEST_EPOCH_MS,
        "America/Vancouver",
    )
    .replace("END:VEVENT", &format!("LOCATION:{location}\r\nEND:VEVENT"))
}

fn ics(body: String, etag: &str) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("ETag", etag)
        .set_body_string(body)
}

#[tokio::test]
async fn bounds_update_and_delete_requests() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(EVENT_PATH))
        .respond_with(ics(server_copy("Front desk"), "\"e1\""))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path(EVENT_PATH))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(EVENT_PATH))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    let w = writer(&server);
    let update = w.update(&session(), &event(), &moved(), UID).await.unwrap();
    let deletion = w.delete(&session(), UID).await.unwrap();
    assert_eq!(
        update,
        UpdateOutcome::Success {
            event_uid: UID.to_owned()
        }
    );
    assert_eq!(deletion, DeleteOutcome::NotFound);
    // An update is conditional on the version it merged into.
    let requests = server.received_requests().await.unwrap();
    let put = &requests[1];
    assert_eq!(put.method.as_str(), "PUT");
    assert!(put.headers.get("if-none-match").is_none());
    assert_eq!(put.headers.get("if-match").unwrap(), "\"e1\"");
    let body = String::from_utf8_lossy(&put.body);
    assert!(body.contains("T100000\r\n"), "{body}");
    assert!(
        body.contains("LOCATION:Front desk\r\n"),
        "manual edit kept: {body}"
    );
}

#[tokio::test]
async fn update_skips_the_write_when_nothing_the_email_owns_changed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ics(server_copy("Front desk"), "\"e1\""))
        .mount(&server)
        .await;
    let result = writer(&server)
        .update(&session(), &event(), &event(), UID)
        .await
        .unwrap();
    assert!(matches!(result, UpdateOutcome::Success { .. }));
    let requests = server.received_requests().await.unwrap();
    assert!(requests.iter().all(|r| r.method.as_str() == "GET"));
}

#[tokio::test]
async fn update_merges_again_after_a_concurrent_edit() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ics(server_copy("Front desk"), "\"e1\""))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .respond_with(ics(server_copy("Room 4"), "\"e2\""))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(header("If-Match", "\"e1\""))
        .respond_with(ResponseTemplate::new(412))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(header("If-Match", "\"e2\""))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    let result = writer(&server)
        .update(&session(), &event(), &moved(), UID)
        .await
        .unwrap();
    assert!(matches!(result, UpdateOutcome::Success { .. }));
    let requests = server.received_requests().await.unwrap();
    let last = requests.last().unwrap();
    let body = String::from_utf8_lossy(&last.body);
    assert!(body.contains("LOCATION:Room 4\r\n"), "{body}");
    assert!(body.contains("T100000\r\n"), "{body}");
}

#[tokio::test]
async fn update_gives_up_after_bounded_conflicts() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ics(server_copy("Front desk"), "\"e1\""))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(412))
        .mount(&server)
        .await;
    let result = writer(&server)
        .update(&session(), &event(), &moved(), UID)
        .await
        .unwrap();
    assert!(
        matches!(result, UpdateOutcome::Error { code: 412, .. }),
        "{result:?}"
    );
    let puts = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.method.as_str() == "PUT")
        .count();
    assert_eq!(puts, MERGE_ATTEMPTS);
}

const UID_CONFLICT: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:error xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <c:no-uid-conflict><d:href>/123/calendars/work/stable@omni-notify.ics</d:href></c:no-uid-conflict>
</d:error>"#;

#[tokio::test]
async fn update_merges_into_an_event_moved_to_another_calendar() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(EVENT_PATH))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    // Recreating at home is refused because the UID now lives in "work".
    Mock::given(method("PUT"))
        .and(path(EVENT_PATH))
        .and(header("If-None-Match", "*"))
        .respond_with(ResponseTemplate::new(403).set_body_string(UID_CONFLICT))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/123/calendars/work/stable@omni-notify.ics"))
        .and(header("Authorization", "Basic secret"))
        .respond_with(ics(server_copy("Front desk"), "\"w1\""))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/123/calendars/work/stable@omni-notify.ics"))
        .and(header("If-Match", "\"w1\""))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    let result = writer(&server)
        .update(&session(), &event(), &moved(), UID)
        .await
        .unwrap();
    assert_eq!(
        result,
        UpdateOutcome::Success {
            event_uid: UID.to_owned()
        }
    );
    let methods: Vec<String> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|r| format!("{} {}", r.method, r.url.path()))
        .collect();
    assert_eq!(
        methods,
        [
            "GET /123/calendars/home/stable@omni-notify.ics",
            "PUT /123/calendars/home/stable@omni-notify.ics",
            "GET /123/calendars/work/stable@omni-notify.ics",
            "PUT /123/calendars/work/stable@omni-notify.ics",
        ]
    );
}

#[tokio::test]
async fn update_403_without_a_uid_conflict_is_an_error_and_deletes_nothing() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(403).set_body_string("<d:error xmlns:d=\"DAV:\"/>"))
        .mount(&server)
        .await;
    let result = writer(&server)
        .update(&session(), &event(), &moved(), UID)
        .await
        .unwrap();
    assert_eq!(
        result,
        UpdateOutcome::Error {
            code: 403,
            message: "CalDAV 403: Forbidden".to_owned()
        }
    );
    let requests = server.received_requests().await.unwrap();
    assert!(requests.iter().all(|r| r.method.as_str() != "DELETE"));
}

#[tokio::test]
async fn update_ignores_a_conflict_on_an_untrusted_host() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(403).set_body_string(
            "<error><no-uid-conflict><href>https://attacker.example/x.ics</href></no-uid-conflict></error>",
        ))
        .mount(&server)
        .await;
    let result = writer(&server)
        .update(&session(), &event(), &moved(), UID)
        .await
        .unwrap();
    assert!(matches!(result, UpdateOutcome::Error { code: 403, .. }));
    // GET (404 by default) and the refused recreate; nothing else.
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn rejects_an_oversized_write_error_response_instead_of_buffering_it() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(
            ResponseTemplate::new(500).set_body_bytes(vec![b'x'; CALDAV_ERROR_MAX_BYTES + 1]),
        )
        .mount(&server)
        .await;
    let error = writer(&server)
        .create(&session(), &event(), UID)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains(&format!(
            "response exceeds the {CALDAV_ERROR_MAX_BYTES}-byte limit"
        )),
        "{error}"
    );
    assert!(!error.transient);
}

#[tokio::test]
async fn reports_http_failures_with_their_status() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(503).set_body_string("busy"))
        .mount(&server)
        .await;
    let result = writer(&server)
        .create(&session(), &event(), UID)
        .await
        .unwrap();
    assert_eq!(
        result,
        CreateOutcome::Error {
            code: 503,
            message: "CalDAV 503: Service Unavailable".to_owned()
        }
    );
}

#[tokio::test]
async fn record_mode_captures_writes_without_sending() {
    let server = MockServer::start().await;
    let w = CaldavWriter::new(
        icloud_http(&server),
        omni_testkit::test_clock(omni_testkit::TEST_EPOCH_MS),
        SideEffectMode::Record,
        "America/Vancouver",
    );
    assert!(matches!(
        w.create(&session(), &event(), UID).await.unwrap(),
        CreateOutcome::Success { .. }
    ));
    assert_eq!(
        w.delete(&session(), UID).await.unwrap(),
        DeleteOutcome::Success
    );
    assert!(server.received_requests().await.unwrap().is_empty());
    let recorded = w.recorded();
    assert_eq!(recorded.len(), 2);
    assert_eq!(recorded[0].method, "PUT");
    assert_eq!(recorded[0].url, format!("{CALENDAR_URL}{UID}.ics"));
    assert!(
        recorded[0]
            .body
            .as_deref()
            .unwrap()
            .contains("UID:stable@omni-notify\r\n")
    );
    assert_eq!(recorded[1].method, "DELETE");
}
