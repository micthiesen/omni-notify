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
use super::merge;
use super::xml::extract_uid_conflict_href;
use crate::error::CaldavError;
use crate::extraction::schema::ExtractedEvent;

const LOG: &str = "CalDAV";

/// Merge attempts for an update that keeps hitting 412.
pub const MERGE_ATTEMPTS: usize = 3;
/// Cap for reading one calendar resource.
const CALDAV_RESOURCE_MAX_BYTES: usize = 256 * 1024;

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

    /// The shared record-mode capture list (the primary calendar tools append
    /// to the same list).
    pub fn recorded_handle(&self) -> Arc<Mutex<Vec<RecordedCaldavWrite>>> {
        self.recorded.clone()
    }

    pub fn mode(&self) -> SideEffectMode {
        self.mode
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
            .put(
                session,
                &url,
                ics.clone(),
                Some("*"),
                None,
                "create calendar event",
            )
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

    /// Updates an existing event without clobbering other edits: reads the
    /// current copy, applies only the field groups that differ between
    /// `base` (what the pipeline last wrote) and `event`, and writes with
    /// `If-Match`. A 412 re-reads and merges again, at most
    /// [`MERGE_ATTEMPTS`] times. A missing resource is recreated with
    /// `If-None-Match: *`; when that is refused with a `no-uid-conflict`
    /// naming a trusted resource in another calendar (a cross-calendar move),
    /// the moved copy is merged in place instead.
    pub async fn update(
        &self,
        session: &CaldavSession,
        base: &ExtractedEvent,
        event: &ExtractedEvent,
        existing_uid: &str,
    ) -> Result<UpdateOutcome, CaldavError> {
        let now_ms = self.clock.now_ms();
        let base_ics = build_icalendar(base, existing_uid, now_ms, &self.default_tz);
        let new_ics = build_icalendar(event, existing_uid, now_ms, &self.default_tz);
        let home = event_url(session, existing_uid)?;
        let mut target = home.clone();
        let success = || {
            Ok(UpdateOutcome::Success {
                event_uid: existing_uid.to_owned(),
            })
        };
        for _ in 0..MERGE_ATTEMPTS {
            let current = self.get(session, &target).await?;
            if current.status == 404 {
                if target != home {
                    // The moved copy vanished too: start over at home.
                    target = home.clone();
                    continue;
                }
                tracing::warn!(target: LOG, "Calendar event {existing_uid} is missing; recreating it");
                let response = self
                    .put(
                        session,
                        &home,
                        new_ics.clone(),
                        Some("*"),
                        None,
                        "recreate calendar event",
                    )
                    .await?;
                if matches!(response.status, 201 | 204) {
                    return success();
                }
                if response.status == 412 {
                    continue;
                }
                let text = http::error_text(&response, "recreate calendar event")?;
                if response.status == 403
                    && let Some(conflict) = self.conflicting_copy(&home, &text)
                {
                    tracing::warn!(
                        target: LOG,
                        "Calendar event {existing_uid} moved to another calendar; updating it there"
                    );
                    target = conflict;
                    continue;
                }
                return Ok(UpdateOutcome::Error {
                    code: response.status,
                    message: status_message(&response),
                });
            }
            if !current.is_ok() {
                return Ok(UpdateOutcome::Error {
                    code: current.status,
                    message: status_message(&current),
                });
            }
            let Some(etag) = current.etag.clone() else {
                return Ok(UpdateOutcome::Error {
                    code: current.status,
                    message: "CalDAV GET returned no ETag".to_owned(),
                });
            };
            let now =
                jiff::Timestamp::from_millisecond(now_ms).unwrap_or(jiff::Timestamp::UNIX_EPOCH);
            let merged = match merge::merge(&current.text(), &base_ics, &new_ics, now) {
                Ok(Some(merged)) => merged,
                Ok(None) => {
                    tracing::info!(target: LOG, "Calendar event already current: {existing_uid}");
                    return success();
                }
                Err(error) => {
                    return Ok(UpdateOutcome::Error {
                        code: 422,
                        message: format!("cannot merge the update: {error}"),
                    });
                }
            };
            tracing::debug!(target: LOG, "CalDAV PUT (merge) {target}\n{merged}");
            let response = self
                .put(
                    session,
                    &target,
                    merged,
                    None,
                    Some(&etag),
                    "update calendar event",
                )
                .await?;
            if matches!(response.status, 200 | 201 | 204) {
                tracing::info!(target: LOG, "Updated calendar event: {} ({existing_uid})", event.title);
                return success();
            }
            if response.status == 412 {
                tracing::info!(target: LOG, "Calendar event {existing_uid} changed meanwhile; merging again");
                continue;
            }
            let text = http::error_text(&response, "update calendar event")?;
            tracing::error!(
                target: LOG,
                "CalDAV PUT (update) failed: {} {} URL: {target}\nResponse:\n{text}",
                response.status,
                response.reason
            );
            return Ok(UpdateOutcome::Error {
                code: response.status,
                message: status_message(&response),
            });
        }
        Ok(UpdateOutcome::Error {
            code: 412,
            message: format!(
                "CalDAV 412: the event kept changing during {MERGE_ATTEMPTS} merge attempts"
            ),
        })
    }

    /// Reads one resource (bounded).
    async fn get(&self, session: &CaldavSession, url: &Url) -> Result<CaldavResponse, CaldavError> {
        http::request(
            &self.http,
            Method::GET,
            url,
            &[("Authorization", session.auth_header.as_str())],
            None,
            "read calendar event",
            CALDAV_RESOURCE_MAX_BYTES,
        )
        .await
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
        if_none_match: Option<&str>,
        if_match: Option<&str>,
        operation: &str,
    ) -> Result<CaldavResponse, CaldavError> {
        if self.mode == SideEffectMode::Record {
            self.record("PUT", url, Some(ics));
            return Ok(CaldavResponse {
                status: 201,
                reason: "Created".to_owned(),
                location: None,
                etag: None,
                body: Vec::new(),
            });
        }
        let mut headers: Vec<(&'static str, &str)> = vec![
            ("Content-Type", "text/calendar; charset=utf-8"),
            ("Authorization", session.auth_header.as_str()),
        ];
        if let Some(value) = if_none_match {
            headers.push(("If-None-Match", value));
        }
        if let Some(value) = if_match {
            headers.push(("If-Match", value));
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
