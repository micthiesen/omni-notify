//! Post-model output sanitization (`src/calendar-events/extraction/sanitize.ts`).
//!
//! The extraction model has degenerated in production (hundreds of identical
//! create objects, paragraphs of field soup in `timeZone`, timed events with no
//! `startTime`), so every model response passes through here before any of it
//! touches CalDAV or persistence.

use std::sync::LazyLock;

use omni_core::js::{utf16_len, utf16_slice};
use regex::Regex;

use super::schema::ExtractedEvent;

pub const MAX_TITLE_CHARS: usize = 200;
pub const MAX_LOCATION_CHARS: usize = 300;
pub const MAX_DESCRIPTION_CHARS: usize = 2000;

/// What [`sanitize_extracted_events`] changed.
#[derive(Clone, Debug, PartialEq)]
pub struct SanitizeResult {
    pub events: Vec<ExtractedEvent>,
    /// Human-readable notes on everything that was fixed (one warn line each).
    pub issues: Vec<String>,
    pub duplicates_collapsed: usize,
    pub time_zones_dropped: usize,
}

/// True if `tz` is a real IANA zone name (aliases such as `Asia/Calcutta` included).
pub fn is_valid_time_zone(tz: &str) -> bool {
    !tz.is_empty() && jiff::tz::TimeZone::get(tz).is_ok()
}

/// A valid zone passes through unchanged; anything else becomes `None` so it can
/// never reach a `DTSTART;TZID=` line or be re-injected into a later prompt.
pub fn sanitize_time_zone(tz: Option<&str>) -> Option<String> {
    tz.filter(|tz| is_valid_time_zone(tz)).map(str::to_owned)
}

/// The first `max` UTF-16 units of `value` (JS `slice`).
pub fn truncated(value: &str, max: usize) -> String {
    if utf16_len(value) > max {
        utf16_slice(value, 0, max).into_owned()
    } else {
        value.to_owned()
    }
}

fn label(event: &ExtractedEvent) -> String {
    format!("\"{}\"", truncated(&event.title, 40))
}

static ISO_DATE: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"^[0-9]{4}-[0-9]{2}-[0-9]{2}$").ok());

pub(crate) fn is_iso_date_shape(value: &str) -> bool {
    ISO_DATE.as_ref().is_some_and(|re| re.is_match(value))
}

/// Collapses byte-identical duplicates, drops invalid time zones, truncates
/// runaway text, forces timeless timed events to all-day and drops malformed
/// recurrences.
pub fn sanitize_extracted_events(events: Vec<ExtractedEvent>) -> SanitizeResult {
    let mut issues = Vec::new();
    let mut time_zones_dropped = 0;
    let raw_count = events.len();
    let mut unique: Vec<ExtractedEvent> = Vec::with_capacity(events.len());
    for event in events {
        if !unique.contains(&event) {
            unique.push(event);
        }
    }
    let duplicates_collapsed = raw_count - unique.len();
    if duplicates_collapsed > 0 {
        issues.push(format!(
            "collapsed {duplicates_collapsed} byte-identical duplicate event(s)"
        ));
    }
    let sanitized = unique
        .into_iter()
        .map(|mut out| {
            if let Some(tz) = out.time_zone.as_deref()
                && !is_valid_time_zone(tz)
            {
                issues.push(format!(
                    "dropped invalid timeZone ({} chars) on {}",
                    utf16_len(tz),
                    label(&out)
                ));
                time_zones_dropped += 1;
                out.time_zone = None;
            }
            let title_len = utf16_len(&out.title);
            if title_len > MAX_TITLE_CHARS {
                issues.push(format!(
                    "truncated title ({title_len} → {MAX_TITLE_CHARS} chars)"
                ));
                out.title = truncated(&out.title, MAX_TITLE_CHARS);
            }
            if let Some(location) = out.location.as_deref() {
                let len = utf16_len(location);
                if len > MAX_LOCATION_CHARS {
                    issues.push(format!(
                        "truncated location ({len} → {MAX_LOCATION_CHARS} chars) on {}",
                        label(&out)
                    ));
                    out.location = Some(truncated(location, MAX_LOCATION_CHARS));
                }
            }
            if let Some(description) = out.description.as_deref() {
                let len = utf16_len(description);
                if len > MAX_DESCRIPTION_CHARS {
                    issues.push(format!(
                        "truncated description ({len} → {MAX_DESCRIPTION_CHARS} chars) on {}",
                        label(&out)
                    ));
                    out.description = Some(truncated(description, MAX_DESCRIPTION_CHARS));
                }
            }
            // A timed event with no startTime would become a midnight event with an
            // 11:30 PM alarm the night before; force it to all-day instead.
            if !out.all_day && out.start_time.is_none() {
                issues.push(format!(
                    "forced allDay on {} (timed event with no startTime)",
                    label(&out)
                ));
                out.all_day = true;
                out.end_time = None;
                out.duration = None;
            }
            if let Some(recurrence) = &out.recurrence
                && !is_iso_date_shape(&recurrence.until)
            {
                issues.push(format!(
                    "dropped recurrence with invalid until \"{}\" on {}",
                    truncated(&recurrence.until, 40),
                    label(&out)
                ));
                out.recurrence = None;
            }
            out
        })
        .collect();
    SanitizeResult {
        events: sanitized,
        issues,
        duplicates_collapsed,
        time_zones_dropped,
    }
}

/// True when the output looks degenerate enough to warrant one fresh retry:
/// more than half the returned objects were duplicates, or a time zone had to
/// be dropped (field soup strongly correlates with a bad sample).
pub fn is_degenerate_extraction(result: &SanitizeResult) -> bool {
    let raw_count = result.events.len() + result.duplicates_collapsed;
    if result.time_zones_dropped > 0 {
        return true;
    }
    raw_count > 0 && result.duplicates_collapsed * 2 > raw_count
}
