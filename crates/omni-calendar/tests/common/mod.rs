//! Shared fixtures: a recording `EmailSupport`, a fake attachment source, mock
//! CalDAV clients and pipeline construction over `omni_testkit::TestApp`.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use omni_calendar::caldav::{Caldav, CaldavSession, CaldavSettings};
use omni_calendar::pipeline::{CalendarEventPipeline, PipelineDeps};
use omni_calendar::support::{
    ActivityEntry, AttachmentSource, CapturedWork, DownloadedAttachment, EmailSupport, RuleVerdict,
    SenderRuleMatch, SupportError, TriageVerdict,
};
use omni_core::email::{EmailAttachment, FetchedEmail};
use omni_http::{HttpClient, SideEffectMode};
use omni_testkit::TestApp;

pub const CALENDAR_URL: &str = "https://p42-caldav.icloud.com/123/calendars/home/";

/// How the fake triage answers.
#[derive(Clone, Debug)]
pub enum Triage {
    Verdict(TriageVerdict),
    Down,
}

#[derive(Default)]
pub struct Recorded {
    pub activity: Vec<ActivityEntry>,
    pub retries: Vec<(String, String, String)>,
    pub classify_calls: usize,
    pub captures: Vec<String>,
}

/// A recording stand-in for the email pipeline core.
pub struct FakeSupport {
    pub rules: Mutex<HashMap<String, SenderRuleMatch>>,
    pub triage: Mutex<Triage>,
    pub triage_cost: Mutex<Option<f64>>,
    pub recorded: Mutex<Recorded>,
}

impl FakeSupport {
    pub fn new(triage: Triage) -> Arc<Self> {
        Arc::new(Self {
            rules: Mutex::new(HashMap::new()),
            triage: Mutex::new(triage),
            triage_cost: Mutex::new(None),
            recorded: Mutex::new(Recorded::default()),
        })
    }

    pub fn calendar_yes() -> Triage {
        Triage::Verdict(TriageVerdict {
            parcel: false,
            calendar: true,
            reason: "upcoming appointment".to_owned(),
        })
    }

    pub fn calendar_no() -> Triage {
        Triage::Verdict(TriageVerdict {
            parcel: false,
            calendar: false,
            reason: "not an event".to_owned(),
        })
    }

    /// A rule matching senders whose address contains `pattern` (the real
    /// matcher lives in `omni_email`; these tests only need containment).
    pub fn add_rule(&self, pattern: &str, verdict: RuleVerdict) {
        self.rules.lock().unwrap().insert(
            pattern.to_owned(),
            SenderRuleMatch {
                pattern: pattern.to_owned(),
                verdict,
            },
        );
    }

    pub fn activity(&self) -> Vec<ActivityEntry> {
        self.recorded.lock().unwrap().activity.clone()
    }

    pub fn retries(&self) -> Vec<(String, String, String)> {
        self.recorded.lock().unwrap().retries.clone()
    }

    pub fn classify_calls(&self) -> usize {
        self.recorded.lock().unwrap().classify_calls
    }
}

impl EmailSupport for FakeSupport {
    fn find_sender_rule<'a>(
        &'a self,
        from: &'a str,
    ) -> BoxFuture<'a, Result<Option<SenderRuleMatch>, SupportError>> {
        let from = from.to_lowercase();
        let found = self
            .rules
            .lock()
            .unwrap()
            .values()
            .find(|rule| from.contains(&rule.pattern))
            .cloned();
        Box::pin(async move { Ok(found) })
    }

    fn classify<'a>(
        &'a self,
        _email: &'a FetchedEmail,
    ) -> BoxFuture<'a, Result<TriageVerdict, SupportError>> {
        self.recorded.lock().unwrap().classify_calls += 1;
        let triage = self.triage.lock().unwrap().clone();
        Box::pin(async move {
            match triage {
                Triage::Verdict(verdict) => Ok(verdict),
                Triage::Down => Err(SupportError::new("model down")),
            }
        })
    }

    fn triage_cost_cents(&self, _email_id: &str) -> Option<f64> {
        *self.triage_cost.lock().unwrap()
    }

    fn record_activity(&self, entry: ActivityEntry) -> BoxFuture<'_, Result<(), SupportError>> {
        self.recorded.lock().unwrap().activity.push(entry);
        Box::pin(async { Ok(()) })
    }

    fn enqueue_retry<'a>(
        &'a self,
        pipeline: &'static str,
        email_id: &'a str,
        reason: &'a str,
    ) -> BoxFuture<'a, Result<(), SupportError>> {
        self.recorded.lock().unwrap().retries.push((
            pipeline.to_owned(),
            email_id.to_owned(),
            reason.to_owned(),
        ));
        Box::pin(async { Ok(()) })
    }

    fn with_log_capture<'a>(
        &'a self,
        activity_id: String,
        _pipeline: &'static str,
        work: CapturedWork<'a>,
    ) -> CapturedWork<'a> {
        self.recorded.lock().unwrap().captures.push(activity_id);
        work
    }
}

/// Serves no attachments.
pub struct NoAttachments;

impl AttachmentSource for NoAttachments {
    fn download<'a>(
        &'a self,
        _attachment: &'a EmailAttachment,
    ) -> BoxFuture<'a, Result<Option<DownloadedAttachment>, SupportError>> {
        Box::pin(async { Ok(None) })
    }
}

pub fn email(id: &str, from: &str, subject: &str, body: &str) -> FetchedEmail {
    FetchedEmail {
        id: id.to_owned(),
        origin: None,
        subject: subject.to_owned(),
        from: from.to_owned(),
        to: None,
        cc: None,
        reply_to: None,
        message_id: None,
        references: None,
        in_reply_to: None,
        text_body: body.to_owned(),
        links: Vec::new(),
        link_metadata: None,
        received_at: "2026-09-01T00:00:00.000Z".to_owned(),
        attachments: Vec::new(),
    }
}

pub fn session() -> CaldavSession {
    CaldavSession {
        calendar_url: CALENDAR_URL.to_owned(),
        auth_header: "Basic secret".to_owned(),
    }
}

/// An offline client whose iCloud origins resolve to the mock server.
pub fn icloud_http(server: &wiremock::MockServer) -> HttpClient {
    omni_testkit::mock_http(
        server,
        &[
            "https://caldav.icloud.com",
            "https://p42-caldav.icloud.com",
            "https://p07-caldav.icloud.com",
        ],
    )
}

pub fn settings(calendar_url: Option<&str>) -> CaldavSettings {
    CaldavSettings {
        username: "user@icloud.com".to_owned(),
        password: "app-password".to_owned(),
        calendar_url: calendar_url.map(str::to_owned),
        calendar_name: None,
    }
}

/// A live-mode CalDAV client against the mock server, using `ICLOUD_CALENDAR_URL`
/// so no discovery PROPFINDs are needed unless `calendar_url` is `None`.
pub fn caldav(app: &TestApp, server: &wiremock::MockServer, calendar_url: Option<&str>) -> Caldav {
    Caldav::new(
        icloud_http(server),
        app.ctx.clock.clone(),
        SideEffectMode::Live,
        Some(settings(calendar_url)),
        "America/Vancouver",
    )
}

pub fn pipeline(
    app: &TestApp,
    caldav: Caldav,
    support: Arc<FakeSupport>,
    ai: omni_ai::Ai,
) -> CalendarEventPipeline {
    CalendarEventPipeline::new(PipelineDeps {
        config: app.ctx.config.clone(),
        store: app.ctx.store.clone(),
        clock: app.ctx.clock.clone(),
        ai,
        pushover: app.ctx.pushover.clone(),
        caldav,
        support,
        attachments: Arc::new(NoAttachments),
    })
}

/// The structured output for a list of events. A strict-mode model returns
/// every key, so optional fields a test leaves out are filled with `null` (the
/// schema rejects absent keys, as zod's `.nullable()` does).
pub fn extraction(mut events: serde_json::Value) -> omni_ai::GenerateResponse {
    const NULLABLE: [&str; 10] = [
        "eventId",
        "endDate",
        "startTime",
        "endTime",
        "duration",
        "location",
        "description",
        "timeZone",
        "recurrence",
        "reminderMinutes",
    ];
    if let Some(items) = events.as_array_mut() {
        for event in items
            .iter_mut()
            .filter_map(serde_json::Value::as_object_mut)
        {
            for key in NULLABLE {
                event.entry(key).or_insert(serde_json::Value::Null);
            }
        }
    }
    omni_ai::GenerateResponse::text(serde_json::json!({ "events": events }).to_string())
}
