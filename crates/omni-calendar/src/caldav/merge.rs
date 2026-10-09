//! Three-way merge for pipeline updates.
//!
//! The pipeline renders the event it last wrote (`base`, from the tracked
//! row) and the event the new email describes (`new`). Only the field groups
//! that differ between those two renders are copied into the current server
//! copy; everything else (edits made in Calendar or by the agent tools,
//! attendees, extra alarms, vendor properties) keeps its original bytes.

use jiff::Timestamp;

use crate::primary::ics::{Component, IcsDoc, Property};
use crate::primary::time::format_utc;

/// The field groups the pipeline owns.
const GROUPS: &[(&str, &[&str])] = &[
    ("title", &["SUMMARY"]),
    ("time", &["DTSTART", "DTEND", "DURATION"]),
    ("location", &["LOCATION"]),
    ("notes", &["DESCRIPTION"]),
    ("recurrence", &["RRULE"]),
];

/// Why a merge could not be planned.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct MergeError(pub String);

fn master(doc: &IcsDoc) -> Option<&Component> {
    doc.events().find(|e| e.prop("RECURRENCE-ID").is_none())
}

fn master_mut(doc: &mut IcsDoc) -> Option<&mut Component> {
    doc.events_mut().find(|e| e.prop("RECURRENCE-ID").is_none())
}

/// Comparable form of a group: `(name, params, value)` per property.
/// `(name, params, value)` of one property.
type PropKey = (String, Vec<(String, Vec<String>)>, String);

fn group(comp: &Component, names: &[&str]) -> Vec<PropKey> {
    comp.props()
        .filter(|p| names.contains(&p.name.as_str()))
        .map(|p| {
            (
                p.name.clone(),
                p.params
                    .iter()
                    .map(|param| (param.name.clone(), param.values.clone()))
                    .collect(),
                p.value.clone(),
            )
        })
        .collect()
}

fn fresh(p: &Property) -> Property {
    let mut out = Property::new(&p.name, p.value.clone());
    out.params = p.params.clone();
    out
}

fn alarm_trigger(comp: &Component) -> Option<String> {
    comp.children()
        .find(|c| c.name == "VALARM")
        .and_then(|a| a.value("TRIGGER").map(str::to_owned))
}

/// The groups that changed between `base` and `new` renders.
pub fn changed_groups(base: &str, new: &str) -> Result<Vec<&'static str>, MergeError> {
    let base = IcsDoc::parse(base).map_err(|e| MergeError(e.0))?;
    let new = IcsDoc::parse(new).map_err(|e| MergeError(e.0))?;
    let (Some(b), Some(n)) = (master(&base), master(&new)) else {
        return Err(MergeError("render has no event".to_owned()));
    };
    let mut out: Vec<&'static str> = GROUPS
        .iter()
        .filter(|(_, names)| group(b, names) != group(n, names))
        .map(|(label, _)| *label)
        .collect();
    if alarm_trigger(b) != alarm_trigger(n) {
        out.push("alarm");
    }
    Ok(out)
}

/// Applies the pipeline's changed groups to the current server copy.
/// Returns `None` when nothing the pipeline owns changed or the current copy
/// already holds the new values.
pub fn merge(
    current: &str,
    base: &str,
    new: &str,
    now: Timestamp,
) -> Result<Option<String>, MergeError> {
    let changed = changed_groups(base, new)?;
    if changed.is_empty() {
        return Ok(None);
    }
    let mut doc = IcsDoc::parse(current)
        .map_err(|e| MergeError(format!("the calendar copy cannot be parsed: {}", e.0)))?;
    let base_doc = IcsDoc::parse(base).map_err(|e| MergeError(e.0))?;
    let new_doc = IcsDoc::parse(new).map_err(|e| MergeError(e.0))?;
    let (Some(base_event), Some(new_event)) = (master(&base_doc), master(&new_doc)) else {
        return Err(MergeError("render has no event".to_owned()));
    };
    let before = doc.clone();
    let Some(target) = master_mut(&mut doc) else {
        return Err(MergeError("the calendar copy has no event".to_owned()));
    };
    let mut time_changed = false;
    for (label, names) in GROUPS {
        if !changed.contains(label) {
            continue;
        }
        let wanted: Vec<Property> = new_event
            .props()
            .filter(|p| names.contains(&p.name.as_str()))
            .map(fresh)
            .collect();
        if group(target, names) == group(new_event, names) {
            continue;
        }
        if *label == "location" {
            target.remove("X-APPLE-STRUCTURED-LOCATION");
        }
        if *label == "time" || *label == "recurrence" {
            time_changed = true;
        }
        let mut wanted = wanted.into_iter();
        // Replace in place where possible to keep property order.
        let first_existing = names.iter().find(|n| target.prop(n).is_some()).copied();
        match (first_existing, wanted.next()) {
            (Some(name), Some(first)) => {
                for n in names.iter().filter(|n| **n != name) {
                    target.remove(n);
                }
                // `set` replaces the first property of the new name; swap the
                // existing name for the wanted one when they differ.
                if first.name != name {
                    target.remove(name);
                    target.insert_prop(first);
                } else {
                    target.set(first);
                }
            }
            (None, Some(first)) => target.insert_prop(first),
            (_, None) => {
                for n in names.iter() {
                    target.remove(n);
                }
            }
        }
        for rest in wanted {
            target.insert_prop(rest);
        }
    }
    if changed.contains(&"alarm") {
        let old_trigger = alarm_trigger(base_event);
        let new_alarm = new_event.children().find(|c| c.name == "VALARM").cloned();
        let mut removed = false;
        if let Some(old) = &old_trigger {
            target.retain_children(|c| {
                if !removed && c.name == "VALARM" && c.value("TRIGGER") == Some(old.as_str()) {
                    removed = true;
                    return false;
                }
                true
            });
        }
        let present = new_alarm.as_ref().is_some_and(|alarm| {
            target
                .children()
                .any(|c| c.name == "VALARM" && c.value("TRIGGER") == alarm.value("TRIGGER"))
        });
        if let Some(alarm) = new_alarm
            && !present
        {
            target.push_child(alarm);
        }
    }
    if doc.serialize() == before.serialize() {
        return Ok(None);
    }
    if let Some(target) = master_mut(&mut doc) {
        let stamp = format_utc(now);
        target.set(Property::new("DTSTAMP", stamp.clone()));
        if target.prop("LAST-MODIFIED").is_some() {
            target.set(Property::new("LAST-MODIFIED", stamp));
        }
        if time_changed {
            let next = target
                .value("SEQUENCE")
                .and_then(|v| v.trim().parse::<i64>().ok())
                .unwrap_or(0)
                + 1;
            target.set(Property::new("SEQUENCE", next.to_string()));
        }
    }
    Ok(Some(doc.serialize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:u\r\nDTSTAMP:20260101T000000Z\r\nSUMMARY:Vet\r\nDTSTART;TZID=America/Vancouver:20261002T163000\r\nDURATION:PT1H\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT30M\r\nEND:VALARM\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    #[test]
    fn copies_only_changed_groups_and_keeps_manual_edits() {
        let new = BASE.replace("163000", "170000");
        let current = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:u\r\nDTSTAMP:20260101T000000Z\r\nSUMMARY:Vet (bring records)\r\nDTSTART;TZID=America/Vancouver:20261002T163000\r\nDURATION:PT1H\r\nLOCATION:Clinic\r\nX-APPLE-TRAVEL-ADVISORY-BEHAVIOR:AUTOMATIC\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT30M\r\nEND:VALARM\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-P1D\r\nEND:VALARM\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let merged = merge(current, BASE, &new, Timestamp::UNIX_EPOCH)
            .unwrap()
            .unwrap();
        assert!(merged.contains("SUMMARY:Vet (bring records)\r\n"));
        assert!(merged.contains("DTSTART;TZID=America/Vancouver:20261002T170000\r\n"));
        assert!(merged.contains("LOCATION:Clinic\r\n"));
        assert!(merged.contains("X-APPLE-TRAVEL-ADVISORY-BEHAVIOR:AUTOMATIC\r\n"));
        assert!(merged.contains("TRIGGER:-P1D\r\n"));
        assert!(merged.contains("SEQUENCE:1\r\n"));
    }

    #[test]
    fn unchanged_renders_merge_to_nothing() {
        assert_eq!(
            merge(BASE, BASE, BASE, Timestamp::UNIX_EPOCH).unwrap(),
            None
        );
    }

    #[test]
    fn replaces_the_pipeline_alarm_only() {
        let new = BASE.replace("TRIGGER:-PT30M", "TRIGGER:-PT1H");
        let merged = merge(BASE, BASE, &new, Timestamp::UNIX_EPOCH)
            .unwrap()
            .unwrap();
        assert!(merged.contains("TRIGGER:-PT1H\r\n"));
        assert!(!merged.contains("TRIGGER:-PT30M"));
    }
}
