//! CalDAV event writes.
//!
//! Transport failures are `Err(CaldavError)`; HTTP-level failures come back as
//! `Error { code, message }` outcomes so callers can classify 5xx as retryable.
//! In [`SideEffectMode::Record`] writes are captured instead of sent.

use std::sync::{Arc, Mutex};

use omni_core::clock::SharedClock;
use omni_http::{HttpClient, Method, SideEffectMode, Url};

use super::http::{self, CALDAV_ERROR_MAX_BYTES, CaldavResponse, assert_trusted_caldav_url};
use super::ics::build_icalendar;
use super::xml::extract_uid_conflict_href;
use crate::error::CaldavError;
use crate::extraction::schema::ExtractedEvent;

const LOG: &str = "CalDAV";

/// Resolved CalDAV target: a calendar collection URL plus the auth to use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaldavSession {
    /// Absolute collection URL, ending with `/`.
    pub calendar_url: String,
    pub auth_header: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CreateOutcome {
    Success {
        event_uid: String,
    },
    /// 412: a resource with this UID already exists (a replayed create).
    AlreadyExists {
        event_uid: String,
    },
    Error {
        code: u16,
        message: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateOutcome {
    Success { event_uid: String },
    Error { code: u16, message: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeleteOutcome {
    Success,
    NotFound,
    Error { code: u16, message: String },
}

/// A write captured in [`SideEffectMode::Record`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedCaldavWrite {
    pub method: &'static str,
    pub url: String,
    pub body: Option<String>,
}

/// Issues CalDAV writes for a session.
#[derive(Clone)]
pub struct CaldavWriter {
    http: HttpClient,
    clock: SharedClock,
    mode: SideEffectMode,
    default_tz: String,
    recorded: Arc<Mutex<Vec<RecordedCaldavWrite>>>,
}

fn status_message(response: &CaldavResponse) -> String {
    format!("CalDAV {}: {}", response.status, response.reason)
}

fn event_url(session: &CaldavSession, uid: &str) -> Result<Url, CaldavError> {
    let raw = format!("{}{uid}.ics", session.calendar_url);
    Url::parse(&raw)
        .map_err(|e| CaldavError::new("build CalDAV event URL", format!("{raw}: {e}"), false))
}

impl CaldavWriter {
    pub fn new(
        http: HttpClient,
        clock: SharedClock,
        mode: SideEffectMode,
        default_tz: impl Into<String>,
    ) -> Self {
        Self {
            http,
            clock,
            mode,
            default_tz: default_tz.into(),
            recorded: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Writes captured in record mode, oldest first.
    pub fn recorded(&self) -> Vec<RecordedCaldavWrite> {
        self.recorded
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    fn record(&self, method: &'static str, url: &Url, body: Option<String>) {
        self.recorded
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(RecordedCaldavWrite {
                method,
                url: url.to_string(),
                body,
            });
    }

    /// The iCalendar body this writer would send now.
    pub fn render(&self, event: &ExtractedEvent, uid: &str) -> String {
        build_icalendar(event, uid, self.clock.now_ms(), &self.default_tz)
    }

    /// PUTs a new event at `<collection><uid>.ics` with `If-None-Match: *`, so a
    /// replay after a lost acknowledgement reports `AlreadyExists` instead of
    /// creating a duplicate.
    pub async fn create(
        &self,
        session: &CaldavSession,
        event: &ExtractedEvent,
        uid: &str,
    ) -> Result<CreateOutcome, CaldavError> {
        let ics = self.render(event, uid);
        let url = event_url(session, uid)?;
        tracing::debug!(target: LOG, "CalDAV PUT {url}\n{ics}");
        let response = self
            .put(session, &url, ics.clone(), true, "create calendar event")
            .await?;
        if matches!(response.status, 201 | 204) {
            tracing::info!(target: LOG, "Created calendar event: {} ({uid})", event.title);
            return Ok(CreateOutcome::Success {
                event_uid: uid.to_owned(),
            });
        }
        if response.status == 412 {
            tracing::info!(target: LOG, "Calendar event already exists: {uid}");
            return Ok(CreateOutcome::AlreadyExists {
                event_uid: uid.to_owned(),
            });
        }
        let text = http::error_text(&response, "create calendar event")?;
        tracing::error!(
            target: LOG,
            "CalDAV PUT failed: {} {} URL: {url}\nBody:\n{ics}\nResponse:\n{text}",
            response.status,
            response.reason
        );
        Ok(CreateOutcome::Error {
            code: response.status,
            message: status_message(&response),
        })
    }

    /// Overwrites an existing event. A 403 that names a `no-uid-conflict`
    /// resource means the event now lives in another calendar (a cross-calendar
    /// move): the conflicting copy is deleted and the event recreated in this
    /// collection. Any other 403 is reported as an error.
    pub async fn update(
        &self,
        session: &CaldavSession,
        event: &ExtractedEvent,
        existing_uid: &str,
    ) -> Result<UpdateOutcome, CaldavError> {
        let ics = self.render(event, existing_uid);
        let url = event_url(session, existing_uid)?;
        tracing::debug!(target: LOG, "CalDAV PUT (update) {url}\n{ics}");
        let response = self
            .put(session, &url, ics.clone(), false, "update calendar event")
            .await?;
        if matches!(response.status, 201 | 204) {
            tracing::info!(target: LOG, "Updated calendar event: {} ({existing_uid})", event.title);
            return Ok(UpdateOutcome::Success {
                event_uid: existing_uid.to_owned(),
            });
        }
        let text = http::error_text(&response, "update calendar event")?;
        if response.status == 403
            && let Some(conflict) = self.conflicting_copy(&url, &text)
        {
            return self
                .recreate_after_move(session, event, existing_uid, &url, &conflict, ics)
                .await;
        }
        tracing::error!(
            target: LOG,
            "CalDAV PUT (update) failed: {} {} URL: {url}\nBody:\n{ics}\nResponse:\n{text}",
            response.status,
            response.reason
        );
        Ok(UpdateOutcome::Error {
            code: response.status,
            message: status_message(&response),
        })
    }

    /// The trusted URL of the resource holding this UID in another collection.
    fn conflicting_copy(&self, url: &Url, body: &str) -> Option<Url> {
        let href = extract_uid_conflict_href(body)?;
        let resolved = url.join(&href).ok()?;
        if resolved == *url {
            return None;
        }
        match assert_trusted_caldav_url(resolved.as_str()) {
            Ok(trusted) => Some(trusted),
            Err(error) => {
                tracing::warn!(target: LOG, "Ignoring UID conflict location: {error}");
                None
            }
        }
    }

    async fn recreate_after_move(
        &self,
        session: &CaldavSession,
        event: &ExtractedEvent,
        uid: &str,
        url: &Url,
        conflict: &Url,
        ics: String,
    ) -> Result<UpdateOutcome, CaldavError> {
        tracing::warn!(
            target: LOG,
            "Calendar event {uid} moved to another calendar ({conflict}); deleting that copy and recreating it here"
        );
        match self
            .delete_url(session, conflict, "delete moved calendar event")
            .await?
        {
            DeleteOutcome::Success | DeleteOutcome::NotFound => {}
            DeleteOutcome::Error { code, message } => {
                return Ok(UpdateOutcome::Error {
                    code,
                    message: format!("{message} (deleting the moved copy)"),
                });
            }
        }
        let response = self
            .put(session, url, ics, false, "recreate calendar event")
            .await?;
        if matches!(response.status, 201 | 204) {
            tracing::info!(target: LOG, "Recreated calendar event: {} ({uid})", event.title);
            return Ok(UpdateOutcome::Success {
                event_uid: uid.to_owned(),
            });
        }
        let text = http::error_text(&response, "recreate calendar event")?;
        tracing::error!(
            target: LOG,
            "CalDAV PUT (recreate) failed: {} {} URL: {url}\nResponse:\n{text}",
            response.status,
            response.reason
        );
        Ok(UpdateOutcome::Error {
            code: response.status,
            message: format!(
                "{} (recreating after the moved copy was deleted)",
                status_message(&response)
            ),
        })
    }

    /// Deletes `<collection><uid>.ics`; 404 is `NotFound`.
    pub async fn delete(
        &self,
        session: &CaldavSession,
        uid: &str,
    ) -> Result<DeleteOutcome, CaldavError> {
        let url = event_url(session, uid)?;
        self.delete_url(session, &url, "delete calendar event")
            .await
    }

    async fn delete_url(
        &self,
        session: &CaldavSession,
        url: &Url,
        operation: &str,
    ) -> Result<DeleteOutcome, CaldavError> {
        tracing::debug!(target: LOG, "CalDAV DELETE {url}");
        if self.mode == SideEffectMode::Record {
            self.record("DELETE", url, None);
            return Ok(DeleteOutcome::Success);
        }
        let response = http::request(
            &self.http,
            Method::DELETE,
            url,
            &[("Authorization", session.auth_header.as_str())],
            None,
            operation,
            CALDAV_ERROR_MAX_BYTES,
        )
        .await?;
        if matches!(response.status, 200 | 204) {
            tracing::info!(target: LOG, "Deleted calendar event: {url}");
            return Ok(DeleteOutcome::Success);
        }
        if response.status == 404 {
            tracing::info!(target: LOG, "Calendar event already gone: {url}");
            return Ok(DeleteOutcome::NotFound);
        }
        let text = http::error_text(&response, operation)?;
        tracing::error!(
            target: LOG,
            "CalDAV DELETE failed: {} {} URL: {url}\nResponse:\n{text}",
            response.status,
            response.reason
        );
        Ok(DeleteOutcome::Error {
            code: response.status,
            message: status_message(&response),
        })
    }

    async fn put(
        &self,
        session: &CaldavSession,
        url: &Url,
        ics: String,
        create_only: bool,
        operation: &str,
    ) -> Result<CaldavResponse, CaldavError> {
        if self.mode == SideEffectMode::Record {
            self.record("PUT", url, Some(ics));
            return Ok(CaldavResponse {
                status: 201,
                reason: "Created".to_owned(),
                location: None,
                body: Vec::new(),
            });
        }
        let mut headers: Vec<(&'static str, &str)> = vec![
            ("Content-Type", "text/calendar; charset=utf-8"),
            ("Authorization", session.auth_header.as_str()),
        ];
        if create_only {
            // Only create, never overwrite.
            headers.push(("If-None-Match", "*"));
        }
        http::request(
            &self.http,
            Method::PUT,
            url,
            &headers,
            Some(ics),
            operation,
            CALDAV_ERROR_MAX_BYTES,
        )
        .await
    }
}
