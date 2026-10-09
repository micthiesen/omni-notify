//! Subsystem wiring: what the app receives with and without CalDAV credentials,
//! and the `CalendarWriter` port.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::sync::Arc;

use common::{FakeSupport, NoAttachments};
use omni_calendar::{CalendarDeps, subsystem};
use omni_runtime::BootPhase;
use omni_runtime::ports::{CalendarCreateOutcome, CalendarEventInput};
use omni_testkit::TestApp;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn deps() -> CalendarDeps {
    CalendarDeps {
        email: FakeSupport::new(FakeSupport::calendar_yes()),
        attachments: Arc::new(NoAttachments),
    }
}

#[tokio::test]
async fn without_credentials_only_tools_and_entities_are_registered() {
    let app = TestApp::new().await;
    let s = subsystem(&app.ctx, deps()).unwrap();
    assert_eq!(s.name, "CalendarEvents");
    assert_eq!(s.mcp_tools.len(), 7);
    assert_eq!(s.entities.len(), 1);
    assert_eq!(s.entities[0].name, "calendar-created-event");
    assert_eq!(s.managed_entities[0].slug, "calendar-created-event");
    assert_eq!(s.managed_entities[0].primary_key, ["eventHash"]);
    assert!(s.email_handlers.is_empty());
    assert!(s.boot_steps.is_empty());
    assert!(s.tasks.is_empty());
    let status = omni_calendar::calendar_writer(&app.ctx).status();
    assert!(!status.configured);
}

#[tokio::test]
async fn with_credentials_the_handler_and_reconcile_step_are_registered() {
    let mut app = TestApp::new().await;
    let mut config = (*app.ctx.config).clone();
    config.icloud_username = Some("user@icloud.com".to_owned());
    config.icloud_app_password = Some("app-password".to_owned());
    app.ctx.config = Arc::new(config);
    let s = subsystem(&app.ctx, deps()).unwrap();
    assert_eq!(s.email_handlers.len(), 1);
    assert_eq!(s.email_handlers[0].name(), "CalendarEvents");
    assert_eq!(s.boot_steps.len(), 1);
    assert_eq!(s.boot_steps[0].phase, BootPhase::Reconcile);
    let step = s.boot_steps.into_iter().next().unwrap();
    (step.run)(app.ctx.clone()).await.unwrap();
}

#[tokio::test]
async fn the_writer_port_creates_with_the_callers_uid_and_maps_412() {
    let mut app = TestApp::new().await;
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(201))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(412))
        .mount(&server)
        .await;
    let mut config = (*app.ctx.config).clone();
    config.icloud_username = Some("user@icloud.com".to_owned());
    config.icloud_app_password = Some("app-password".to_owned());
    config.icloud_calendar_url = Some(common::CALENDAR_URL.to_owned());
    app.ctx.config = Arc::new(config);
    app.ctx.http = common::icloud_http(&server);
    app.ctx.side_effects = omni_http::SideEffectMode::Live;
    let writer = omni_calendar::calendar_writer(&app.ctx);
    let input = CalendarEventInput {
        title: "Pickup".to_owned(),
        start_date: "2026-09-02".to_owned(),
        end_date: None,
        start_time: Some("18:00".to_owned()),
        end_time: None,
        location: None,
        description: None,
        time_zone: None,
        all_day: false,
        reminder_minutes: None,
    };
    let uid = "workspace-a1@omni-notify";
    assert_eq!(
        writer.create_event(uid, &input).await.unwrap(),
        CalendarCreateOutcome::Created {
            event_uid: uid.to_owned()
        }
    );
    assert_eq!(
        writer.create_event(uid, &input).await.unwrap(),
        CalendarCreateOutcome::AlreadyExists
    );
    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests[0].url.path(),
        "/123/calendars/home/workspace-a1@omni-notify.ics"
    );
    assert!(writer.status().configured);
}

#[tokio::test]
async fn record_mode_never_reaches_caldav() {
    let mut app = TestApp::new().await;
    let server = MockServer::start().await;
    let mut config = (*app.ctx.config).clone();
    config.icloud_username = Some("user@icloud.com".to_owned());
    config.icloud_app_password = Some("app-password".to_owned());
    config.icloud_calendar_url = Some(common::CALENDAR_URL.to_owned());
    app.ctx.config = Arc::new(config);
    app.ctx.http = common::icloud_http(&server);
    let writer = omni_calendar::calendar_writer(&app.ctx);
    let input = CalendarEventInput {
        title: "Pickup".to_owned(),
        start_date: "2026-09-02".to_owned(),
        end_date: None,
        start_time: None,
        end_time: None,
        location: None,
        description: None,
        time_zone: None,
        all_day: true,
        reminder_minutes: None,
    };
    assert!(matches!(
        writer
            .create_event("workspace-a2@omni-notify", &input)
            .await
            .unwrap(),
        CalendarCreateOutcome::Created { .. }
    ));
    assert!(server.received_requests().await.unwrap().is_empty());
}

/// Never consulted: the subsystem is only built here.
struct UnusedClassifier;

impl omni_email::triage::TriageClassifier for UnusedClassifier {
    fn classify(
        &self,
        email: omni_email::triage::TriageEmail,
    ) -> futures::future::BoxFuture<
        'static,
        Result<omni_email::triage::Classified, omni_email::triage::TriageError>,
    > {
        Box::pin(async move {
            Err(omni_email::triage::TriageError {
                email_id: email.id,
                message: "unused".to_owned(),
            })
        })
    }
}

#[tokio::test]
async fn production_deps_wire_the_handler_over_the_email_core() {
    let mut app = TestApp::new().await;
    let mut config = (*app.ctx.config).clone();
    config.icloud_username = Some("user@icloud.com".to_owned());
    config.icloud_app_password = Some("app-password".to_owned());
    app.ctx.config = Arc::new(config);
    let deps = CalendarDeps::new(
        &app.ctx,
        omni_email::triage::EmailTriage::new(Arc::new(UnusedClassifier)),
    );
    let s = subsystem(&app.ctx, deps).unwrap();
    assert_eq!(s.email_handlers.len(), 1);
    assert_eq!(s.email_handlers[0].name(), "CalendarEvents");
}
