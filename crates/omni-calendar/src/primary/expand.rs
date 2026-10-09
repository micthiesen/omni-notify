//! Client-side recurrence expansion of one resource into occurrences.
//!
//! The series is expanded in civil time in the master's zone, RDATEs are
//! added and EXDATEs removed by canonical key, override VEVENTs replace their
//! instances, and each instance resolves to an instant through jiff (a gap
//! moves forward, a fold takes the earlier instant).

use std::collections::{BTreeMap, BTreeSet};

use jiff::civil::{Date, DateTime};
use jiff::{SignedDuration, Timestamp};

use super::ics::IcsDoc;
use super::model::{self, EventSpan, EventView, SchedulingRole};
use super::time::{EventTime, Zones};

/// Iteration bound per master per request.
pub const MAX_ITERATIONS: usize = 10_000;

/// One expanded occurrence.
#[derive(Clone, Debug, PartialEq)]
pub struct Occurrence {
    pub uid: String,
    /// Canonical key of the original start; `None` for a non-recurring event.
    pub recurrence_id: Option<String>,
    pub is_override: bool,
    pub title: String,
    pub start: EventTime,
    pub end: EventTime,
    pub start_utc: Timestamp,
    pub end_utc: Timestamp,
    pub all_day: bool,
    pub last_date: Option<Date>,
    pub location: Option<String>,
    pub notes: Option<String>,
    pub recurring: bool,
    pub has_alarms: bool,
    /// The occurrence's own alarms (an override's, else the master's).
    pub alarms: Vec<model::AlarmView>,
    pub scheduling_role: SchedulingRole,
    pub free: bool,
    pub status: Option<String>,
    pub tz_rules_differ: bool,
}

/// Occurrences of a resource overlapping a window, plus expansion flags.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Expanded {
    pub occurrences: Vec<Occurrence>,
    pub truncated: bool,
    pub unsupported: bool,
}

struct Ctx<'a, 'z> {
    zones: &'a Zones<'z>,
    owner: &'a BTreeSet<String>,
}

fn occurrence_of(
    ctx: &Ctx<'_, '_>,
    event: EventView<'_>,
    master: Option<EventView<'_>>,
    start: EventTime,
    recurrence_id: Option<String>,
    span: EventSpan,
    end_like: Option<&EventTime>,
) -> Option<Occurrence> {
    let start_utc = ctx.zones.instant(&start)?;
    let end = span.end_for(&start, end_like, ctx.zones)?;
    let end_utc = ctx.zones.instant(&end)?;
    let all_day = start.is_date();
    let recurring = master.is_some_and(|m| !m.rrules().is_empty() || !m.rdates().is_empty())
        || recurrence_id.is_some();
    Some(Occurrence {
        uid: event.uid().unwrap_or_default(),
        is_override: event.comp.prop("RECURRENCE-ID").is_some(),
        recurrence_id,
        title: event.summary().unwrap_or_default(),
        last_date: match &start {
            EventTime::Date(d) => Some(model::last_date(*d, Some(&end))),
            _ => None,
        },
        tz_rules_differ: ctx.zones.rules_differ(&start),
        start,
        end,
        start_utc,
        end_utc: end_utc.max(start_utc),
        all_day,
        location: event.location(),
        notes: event.description(),
        recurring,
        has_alarms: !event.alarms().is_empty(),
        alarms: event.alarms(),
        scheduling_role: master.unwrap_or(event).scheduling_role(ctx.owner),
        free: event.is_free(),
        status: event.status(),
    })
}

fn overlaps(o: &Occurrence, from: Timestamp, to: Timestamp) -> bool {
    if o.end_utc > o.start_utc {
        o.start_utc < to && o.end_utc > from
    } else {
        o.start_utc >= from && o.start_utc < to
    }
}

/// The canonical keys of a series' instances up to `until_civil` (inclusive),
/// after RDATE and EXDATE, plus the expansion flags.
pub fn instance_keys(
    doc: &IcsDoc,
    zones: &Zones<'_>,
    until_civil: DateTime,
) -> (Vec<(String, DateTime)>, bool, bool) {
    let Some(master) = model::master(doc) else {
        return (Vec::new(), false, false);
    };
    let Some(start) = master.start() else {
        return (Vec::new(), false, false);
    };
    series_instances(&master, &start, zones, None, until_civil, true)
}

/// Raw series instances (RRULE and RDATE, EXDATEs not applied) as
/// `(key, civil start)` up to `until_civil`, plus the truncated and
/// unsupported flags.
pub fn raw_instances(
    master: &EventView<'_>,
    start: &EventTime,
    zones: &Zones<'_>,
    until_civil: DateTime,
) -> (Vec<(String, DateTime)>, bool, bool) {
    series_instances(master, start, zones, None, until_civil, false)
}

/// Series instances as `(key, civil start)` sorted by civil start.
fn series_instances(
    master: &EventView<'_>,
    start: &EventTime,
    zones: &Zones<'_>,
    skip_to: Option<DateTime>,
    until_civil: DateTime,
    apply_exdates: bool,
) -> (Vec<(String, DateTime)>, bool, bool) {
    let mut civil: BTreeMap<DateTime, ()> = BTreeMap::new();
    let mut truncated = false;
    let mut unsupported = false;
    let base = start.civil();
    match master.rrule() {
        Some(rule) => {
            let expansion = rule.expand(base, skip_to, until_civil, MAX_ITERATIONS, |dt| {
                rule.until_allows(dt, |d| zones.instant(&start.with_civil(d)))
            });
            truncated = expansion.truncated;
            unsupported = expansion.unsupported || master.rrules().len() > 1;
            for dt in expansion.instances {
                civil.insert(dt, ());
            }
        }
        None => {
            if base <= until_civil {
                civil.insert(base, ());
            }
        }
    }
    for rdate in master.rdates() {
        if let Some(dt) = zones.to_master_civil(&rdate, start)
            && dt <= until_civil
        {
            civil.insert(dt, ());
        }
    }
    let excluded: BTreeSet<String> = if apply_exdates {
        master
            .exdates()
            .iter()
            .filter_map(|ex| zones.key(ex, start))
            .collect()
    } else {
        BTreeSet::new()
    };
    let keys = civil
        .into_keys()
        .map(|dt| (super::time::format_key(start, dt), dt))
        .filter(|(key, _)| !excluded.contains(key))
        .collect();
    (keys, truncated, unsupported)
}

/// Every occurrence of the resource overlapping `[from, to)`.
pub fn expand(
    doc: &IcsDoc,
    zones: &Zones<'_>,
    owner: &BTreeSet<String>,
    from: Timestamp,
    to: Timestamp,
) -> Expanded {
    let ctx = Ctx { zones, owner };
    let mut out = Expanded::default();
    let master = model::master(doc);
    let overrides = model::overrides(doc);
    let Some(master_view) = master else {
        // Orphan overrides (an invitation to single instances).
        for o in overrides {
            let Some(start) = o.start() else { continue };
            let key = o.recurrence_id().and_then(|rid| zones.key(&rid, &rid));
            let span = o.span(zones);
            if let Some(occ) = occurrence_of(&ctx, o, None, start, key, span, o.dtend().as_ref())
                && overlaps(&occ, from, to)
            {
                out.occurrences.push(occ);
            }
        }
        return out;
    };
    let Some(start) = master_view.start() else {
        return out;
    };
    let span = master_view.span(zones);
    let dtend = master_view.dtend();
    let recurring = !master_view.rrules().is_empty() || !master_view.rdates().is_empty();
    if !recurring {
        if let Some(occ) = occurrence_of(
            &ctx,
            master_view,
            Some(master_view),
            start,
            None,
            span,
            dtend.as_ref(),
        ) && overlaps(&occ, from, to)
        {
            out.occurrences.push(occ);
        }
        return out;
    }
    let longest = match span {
        EventSpan::Days(d) => SignedDuration::from_hours(24 * (d + 1)),
        EventSpan::Exact(d) => d,
    };
    let reference = |ts: Timestamp| zones.civil_in(ts, &start);
    let skip_to = from
        .checked_sub(longest)
        .ok()
        .and_then(|ts| ts.checked_sub(SignedDuration::from_hours(48)).ok())
        .map(reference);
    let until_civil = to
        .checked_add(SignedDuration::from_hours(48))
        .ok()
        .map_or_else(|| reference(to), reference);
    let (instances, truncated, unsupported) =
        series_instances(&master_view, &start, zones, skip_to, until_civil, true);
    out.truncated = truncated;
    out.unsupported = unsupported;
    let mut by_key: BTreeMap<String, EventView<'_>> = BTreeMap::new();
    let mut shifts: Vec<(DateTime, SignedDuration)> = Vec::new();
    for o in &overrides {
        let Some(rid) = o.recurrence_id() else {
            continue;
        };
        let Some(key) = zones.key(&rid, &start) else {
            continue;
        };
        if o.this_and_future()
            && let (Some(orig), Some(new_start)) = (zones.to_master_civil(&rid, &start), o.start())
            && let (Some(a), Some(b)) = (zones.instant(&rid), zones.instant(&new_start))
        {
            shifts.push((orig, b.duration_since(a)));
        }
        by_key.insert(key, *o);
    }
    shifts.sort_by_key(|(at, _)| *at);
    for (key, civil) in instances {
        if by_key.contains_key(&key) {
            continue;
        }
        let shift = shifts
            .iter()
            .rev()
            .find(|(at, _)| *at < civil)
            .map(|(_, d)| *d);
        let instance_start = start.with_civil(civil);
        let instance_start = match shift {
            Some(delta) => match zones
                .instant(&instance_start)
                .and_then(|i| i.checked_add(delta).ok())
            {
                Some(shifted) => start.with_civil(zones.civil_in(shifted, &start)),
                None => instance_start,
            },
            None => instance_start,
        };
        if let Some(occ) = occurrence_of(
            &ctx,
            master_view,
            Some(master_view),
            instance_start,
            Some(key),
            span,
            dtend.as_ref(),
        ) && overlaps(&occ, from, to)
        {
            out.occurrences.push(occ);
        }
    }
    let excluded: BTreeSet<String> = master_view
        .exdates()
        .iter()
        .filter_map(|ex| zones.key(ex, &start))
        .collect();
    for (key, o) in by_key {
        if excluded.contains(&key) {
            continue;
        }
        let Some(o_start) = o.start() else { continue };
        let o_span = o.span(zones);
        if let Some(occ) = occurrence_of(
            &ctx,
            o,
            Some(master_view),
            o_start,
            Some(key),
            o_span,
            o.dtend().as_ref(),
        ) && overlaps(&occ, from, to)
        {
            out.occurrences.push(occ);
        }
    }
    out.occurrences.sort_by(|a, b| {
        a.start_utc
            .cmp(&b.start_utc)
            .then(a.recurrence_id.cmp(&b.recurrence_id))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::tz::TimeZone;

    fn zones(doc: &IcsDoc) -> Zones<'_> {
        Zones::new(doc, TimeZone::get("America/Vancouver").unwrap())
    }

    const WEEKLY: &str = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:w\r\nSUMMARY:Standup\r\nDTSTART;TZID=America/Vancouver:20261005T090000\r\nDTEND;TZID=America/Vancouver:20261005T093000\r\nRRULE:FREQ=WEEKLY;COUNT=6\r\nEXDATE;TZID=America/Vancouver:20261012T090000\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:w\r\nRECURRENCE-ID;TZID=America/Vancouver:20261019T090000\r\nSUMMARY:Standup (moved)\r\nDTSTART;TZID=America/Vancouver:20261020T100000\r\nDTEND;TZID=America/Vancouver:20261020T103000\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    #[test]
    fn exdate_and_override() {
        let doc = IcsDoc::parse(WEEKLY).unwrap();
        let z = zones(&doc);
        let out = expand(
            &doc,
            &z,
            &BTreeSet::new(),
            "2026-10-01T00:00:00Z".parse().unwrap(),
            "2026-12-01T00:00:00Z".parse().unwrap(),
        );
        let got: Vec<(String, String)> = out
            .occurrences
            .iter()
            .map(|o| (o.recurrence_id.clone().unwrap(), o.title.clone()))
            .collect();
        assert_eq!(
            got,
            [
                (
                    "2026-10-05T09:00:00[America/Vancouver]".into(),
                    "Standup".into()
                ),
                (
                    "2026-10-19T09:00:00[America/Vancouver]".into(),
                    "Standup (moved)".into()
                ),
                (
                    "2026-10-26T09:00:00[America/Vancouver]".into(),
                    "Standup".into()
                ),
                (
                    "2026-11-02T09:00:00[America/Vancouver]".into(),
                    "Standup".into()
                ),
                (
                    "2026-11-09T09:00:00[America/Vancouver]".into(),
                    "Standup".into()
                ),
            ]
        );
        assert_eq!(
            out.occurrences[1].start.civil().to_string(),
            "2026-10-20T10:00:00"
        );
    }

    #[test]
    fn multi_day_all_day() {
        let doc = IcsDoc::parse("BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:h\r\nSUMMARY:Hotel\r\nDTSTART;VALUE=DATE:20261010\r\nDTEND;VALUE=DATE:20261013\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n").unwrap();
        let z = zones(&doc);
        let out = expand(
            &doc,
            &z,
            &BTreeSet::new(),
            "2026-10-12T12:00:00Z".parse().unwrap(),
            "2026-10-12T13:00:00Z".parse().unwrap(),
        );
        assert_eq!(out.occurrences.len(), 1);
        assert!(out.occurrences[0].all_day);
        assert_eq!(
            out.occurrences[0].last_date,
            Some("2026-10-12".parse().unwrap())
        );
    }
}
