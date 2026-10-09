//! Sanitization of extracted calendar events.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_calendar::extraction::sanitize::{
    MAX_DESCRIPTION_CHARS, MAX_LOCATION_CHARS, MAX_TITLE_CHARS, is_degenerate_extraction,
    is_valid_time_zone, sanitize_extracted_events, sanitize_time_zone, truncated,
};
use omni_calendar::extraction::schema::{
    EventAction, EventRecurrence, ExtractedEvent, RecurrenceFrequency,
};

fn evt() -> ExtractedEvent {
    let mut e = ExtractedEvent::new(
        EventAction::Create,
        "🦷 Dentist Appointment",
        "2026-06-17",
        false,
    );
    e.start_time = Some("14:30".to_owned());
    e
}

fn titled(title: &str) -> ExtractedEvent {
    ExtractedEvent {
        title: title.to_owned(),
        ..evt()
    }
}

// describe("isValidTimeZone")

#[test]
fn accepts_canonical_iana_zones() {
    assert!(is_valid_time_zone("America/Vancouver"));
    assert!(is_valid_time_zone("Europe/London"));
}

#[test]
fn accepts_resolvable_aliases_outside_supported_values_of() {
    assert!(is_valid_time_zone("Asia/Calcutta"));
}

#[test]
fn rejects_garbage_including_model_field_soup() {
    assert!(!is_valid_time_zone("America/Nowhere"));
    assert!(!is_valid_time_zone(
        &"The event takes place at the community centre startTime 09:00 endTime 16:00 ".repeat(50)
    ));
}

// describe("sanitizeTimeZone")

#[test]
fn passes_valid_zones_through_and_drops_invalid_ones() {
    assert_eq!(
        sanitize_time_zone(Some("America/Toronto")).as_deref(),
        Some("America/Toronto")
    );
    assert_eq!(
        sanitize_time_zone(Some("not a zone; startDate 2026-07-06")),
        None
    );
    assert_eq!(sanitize_time_zone(None), None);
}

// describe("sanitizeExtractedEvents")

#[test]
fn drops_an_invalid_timezone_and_reports_it() {
    let soup = "paragraph of field soup ".repeat(160);
    let result = sanitize_extracted_events(vec![ExtractedEvent {
        time_zone: Some(soup),
        ..evt()
    }]);
    assert_eq!(result.events[0].time_zone, None);
    assert_eq!(result.time_zones_dropped, 1);
    assert!(result.issues.iter().any(|i| i.contains("invalid timeZone")));
}

#[test]
fn keeps_a_valid_timezone_untouched() {
    let result = sanitize_extracted_events(vec![ExtractedEvent {
        time_zone: Some("America/Vancouver".to_owned()),
        ..evt()
    }]);
    assert_eq!(
        result.events[0].time_zone.as_deref(),
        Some("America/Vancouver")
    );
    assert_eq!(result.time_zones_dropped, 0);
    assert!(result.issues.is_empty());
}

#[test]
fn truncates_over_long_title_location_and_description() {
    let result = sanitize_extracted_events(vec![ExtractedEvent {
        title: "T".repeat(MAX_TITLE_CHARS + 50),
        location: Some("L".repeat(MAX_LOCATION_CHARS + 50)),
        description: Some("D".repeat(MAX_DESCRIPTION_CHARS + 50)),
        ..evt()
    }]);
    let event = &result.events[0];
    assert_eq!(event.title.len(), MAX_TITLE_CHARS);
    assert_eq!(event.location.as_deref().unwrap().len(), MAX_LOCATION_CHARS);
    assert_eq!(
        event.description.as_deref().unwrap().len(),
        MAX_DESCRIPTION_CHARS
    );
    assert_eq!(result.issues.len(), 3);
}

#[test]
fn truncation_counts_utf16_units_like_js() {
    // Each emoji is two UTF-16 units; the cut lands between surrogates like JS.
    let result = sanitize_extracted_events(vec![titled(&"😀".repeat(150))]);
    assert_eq!(
        omni_core::js::utf16_len(&result.events[0].title),
        MAX_TITLE_CHARS
    );
    assert_eq!(result.issues, vec!["truncated title (300 → 200 chars)"]);
}

#[test]
fn collapses_byte_identical_duplicates_to_one_degenerate_repetition() {
    let result = sanitize_extracted_events(vec![evt(); 100]);
    assert_eq!(result.events.len(), 1);
    assert_eq!(result.duplicates_collapsed, 99);
    assert!(
        result
            .issues
            .iter()
            .any(|i| i.contains("99 byte-identical"))
    );
}

#[test]
fn keeps_distinct_events_intact_when_collapsing_duplicates() {
    let result = sanitize_extracted_events(vec![titled("A"), titled("B"), titled("A")]);
    let titles: Vec<&str> = result.events.iter().map(|e| e.title.as_str()).collect();
    assert_eq!(titles, ["A", "B"]);
    assert_eq!(result.duplicates_collapsed, 1);
}

#[test]
fn forces_allday_and_clears_time_fields_for_a_timed_event_with_no_starttime() {
    let result = sanitize_extracted_events(vec![ExtractedEvent {
        start_time: None,
        end_time: Some("16:00".to_owned()),
        duration: Some("PT2H".to_owned()),
        all_day: false,
        ..evt()
    }]);
    let event = &result.events[0];
    assert!(event.all_day);
    assert_eq!(event.start_time, None);
    assert_eq!(event.end_time, None);
    assert_eq!(event.duration, None);
    assert!(result.issues.iter().any(|i| i.contains("forced allDay")));
}

#[test]
fn leaves_a_genuine_all_day_event_alone() {
    let result = sanitize_extracted_events(vec![ExtractedEvent {
        start_time: None,
        all_day: true,
        ..evt()
    }]);
    assert!(result.events[0].all_day);
    assert!(result.issues.is_empty());
}

#[test]
fn normalizes_recurrence_null_to_undefined_without_reporting_an_issue() {
    // The decoder maps a model `null` to `None`, so nothing is left to report.
    let parsed: ExtractedEvent = serde_json::from_value(serde_json::json!({
        "action": "create", "title": "X", "startDate": "2026-06-17", "startTime": "14:30",
        "allDay": false, "recurrence": null
    }))
    .unwrap();
    let result = sanitize_extracted_events(vec![parsed]);
    assert_eq!(result.events[0].recurrence, None);
    assert!(result.issues.is_empty());
}

#[test]
fn keeps_a_valid_recurrence_and_drops_one_with_a_malformed_until_date() {
    let valid = ExtractedEvent {
        recurrence: Some(EventRecurrence {
            frequency: RecurrenceFrequency::Daily,
            until: "2026-07-13".to_owned(),
        }),
        ..evt()
    };
    let invalid = ExtractedEvent {
        title: "Other".to_owned(),
        recurrence: Some(EventRecurrence {
            frequency: RecurrenceFrequency::Daily,
            until: "sometime in July".to_owned(),
        }),
        ..evt()
    };
    let result = sanitize_extracted_events(vec![valid, invalid]);
    assert_eq!(
        result.events[0].recurrence,
        Some(EventRecurrence {
            frequency: RecurrenceFrequency::Daily,
            until: "2026-07-13".to_owned()
        })
    );
    assert_eq!(result.events[1].recurrence, None);
    assert!(result.issues.iter().any(|i| i.contains("invalid until")));
}

#[test]
fn returns_an_empty_result_for_empty_input() {
    let result = sanitize_extracted_events(Vec::new());
    assert!(result.events.is_empty());
    assert!(result.issues.is_empty());
}

// describe("isDegenerateExtraction")

#[test]
fn flags_any_dropped_timezone() {
    let result = sanitize_extracted_events(vec![ExtractedEvent {
        time_zone: Some("garbage soup".to_owned()),
        ..evt()
    }]);
    assert!(is_degenerate_extraction(&result));
}

#[test]
fn flags_collapsing_more_than_half_the_returned_objects() {
    let result = sanitize_extracted_events(vec![evt(), evt(), evt()]);
    assert_eq!(result.duplicates_collapsed, 2);
    assert!(is_degenerate_extraction(&result));
}

#[test]
fn does_not_flag_a_clean_extraction_or_mild_duplication() {
    let clean = sanitize_extracted_events(vec![evt(), titled("Other")]);
    assert!(!is_degenerate_extraction(&clean));
    let mild = sanitize_extracted_events(vec![evt(), evt(), titled("B"), titled("C")]);
    assert_eq!(mild.duplicates_collapsed, 1);
    assert!(!is_degenerate_extraction(&mild));
}

#[test]
fn does_not_flag_truncation_only_fixes() {
    let result = sanitize_extracted_events(vec![titled(&"T".repeat(MAX_TITLE_CHARS + 1))]);
    assert!(!is_degenerate_extraction(&result));
}

// describe("truncated")

#[test]
fn truncates_only_when_over_the_cap() {
    assert_eq!(truncated("short", 10), "short");
    assert_eq!(truncated("0123456789abc", 10), "0123456789");
}
