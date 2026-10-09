//! Typed views over the VEVENTs of one calendar resource.

use std::collections::BTreeSet;

use jiff::civil::Date;
use jiff::{SignedDuration, Timestamp, ToSpan as _};
use serde::{Deserialize, Serialize};

use super::ics::{Component, IcsDoc};
use super::rrule::RRule;
use super::time::{EventTime, Zones};

/// How Michael relates to an event's attendees (RFC 6638 implicit
/// scheduling decides who gets email when it is written).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SchedulingRole {
    None,
    Organizer,
    Attendee,
}

/// An iCalendar DURATION (`[+-]P[nW][nD][T[nH][nM][nS]]`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IcalDuration {
    pub negative: bool,
    pub days: i64,
    pub seconds: i64,
}

impl IcalDuration {
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        let (negative, rest) = match value.as_bytes().first()? {
            b'-' => (true, &value[1..]),
            b'+' => (false, &value[1..]),
            _ => (false, value),
        };
        let rest = rest.strip_prefix(['P', 'p'])?;
        let (date, time) = match rest.split_once(['T', 't']) {
            Some((d, t)) => (d, Some(t)),
            None => (rest, None),
        };
        let mut days = 0i64;
        let mut seconds = 0i64;
        let mut any = false;
        let mut number = String::new();
        for c in date.chars() {
            if c.is_ascii_digit() {
                number.push(c);
                continue;
            }
            let n: i64 = number.parse().ok()?;
            number.clear();
            any = true;
            match c.to_ascii_uppercase() {
                'W' => days += n * 7,
                'D' => days += n,
                _ => return None,
            }
        }
        if !number.is_empty() {
            return None;
        }
        if let Some(time) = time {
            for c in time.chars() {
                if c.is_ascii_digit() {
                    number.push(c);
                    continue;
                }
                let n: i64 = number.parse().ok()?;
                number.clear();
                any = true;
                match c.to_ascii_uppercase() {
                    'H' => seconds += n * 3600,
                    'M' => seconds += n * 60,
                    'S' => seconds += n,
                    _ => return None,
                }
            }
            if !number.is_empty() {
                return None;
            }
        }
        any.then_some(Self {
            negative,
            days,
            seconds,
        })
    }

    pub fn signed_seconds(&self) -> i64 {
        let total = self.days * 86_400 + self.seconds;
        if self.negative { -total } else { total }
    }

    /// Minutes, negative before the anchor.
    pub fn minutes(&self) -> i64 {
        self.signed_seconds() / 60
    }

    /// `PT..M` / `-PT..M` / `P..D` text for whole minutes.
    pub fn from_minutes(minutes: i64) -> String {
        let sign = if minutes < 0 { "-" } else { "" };
        let abs = minutes.abs();
        if abs != 0 && abs % 1440 == 0 {
            return format!("{sign}P{}D", abs / 1440);
        }
        if abs != 0 && abs % 60 == 0 {
            return format!("{sign}PT{}H", abs / 60);
        }
        format!("{sign}PT{abs}M")
    }
}

/// A VALARM's trigger.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trigger {
    /// Minutes relative to the start (negative = before).
    Start(i64),
    End(i64),
    At(Timestamp),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlarmView {
    pub id: Option<String>,
    pub action: String,
    pub trigger: Option<Trigger>,
    pub trigger_text: String,
    /// When a device last dismissed the alarm (`ACKNOWLEDGED`).
    pub acknowledged: Option<Timestamp>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Participant {
    pub email: Option<String>,
    pub name: Option<String>,
    pub role: Option<String>,
    pub partstat: Option<String>,
}

/// Lowercased address without `mailto:`.
pub fn mail_address(value: &str) -> Option<String> {
    let value = value.trim();
    let lower = value.to_ascii_lowercase();
    let address = lower.strip_prefix("mailto:").unwrap_or(&lower).trim();
    (!address.is_empty() && address.contains('@')).then(|| address.to_owned())
}

/// One VEVENT component.
#[derive(Clone, Copy, Debug)]
pub struct EventView<'a> {
    pub comp: &'a Component,
}

impl<'a> EventView<'a> {
    pub fn uid(&self) -> Option<String> {
        self.comp.value("UID").map(|v| v.trim().to_owned())
    }

    pub fn summary(&self) -> Option<String> {
        self.comp.text("SUMMARY")
    }

    pub fn description(&self) -> Option<String> {
        self.comp.text("DESCRIPTION")
    }

    pub fn location(&self) -> Option<String> {
        self.comp.text("LOCATION")
    }

    pub fn url(&self) -> Option<String> {
        self.comp.value("URL").map(|v| v.trim().to_owned())
    }

    pub fn start(&self) -> Option<EventTime> {
        self.comp.prop("DTSTART").and_then(EventTime::parse_first)
    }

    pub fn dtend(&self) -> Option<EventTime> {
        self.comp.prop("DTEND").and_then(EventTime::parse_first)
    }

    pub fn duration(&self) -> Option<IcalDuration> {
        self.comp.value("DURATION").and_then(IcalDuration::parse)
    }

    pub fn recurrence_id(&self) -> Option<EventTime> {
        self.comp
            .prop("RECURRENCE-ID")
            .and_then(EventTime::parse_first)
    }

    pub fn this_and_future(&self) -> bool {
        self.comp
            .prop("RECURRENCE-ID")
            .and_then(|p| p.param("RANGE"))
            .is_some_and(|r| r.eq_ignore_ascii_case("THISANDFUTURE"))
    }

    pub fn rrules(&self) -> Vec<String> {
        self.comp
            .props_named("RRULE")
            .map(|p| p.value.trim().to_owned())
            .collect()
    }

    pub fn rrule(&self) -> Option<RRule> {
        let rules = self.rrules();
        rules.first().and_then(|r| RRule::parse(r).ok())
    }

    pub fn exdates(&self) -> Vec<EventTime> {
        self.comp
            .props_named("EXDATE")
            .flat_map(EventTime::parse_property)
            .collect()
    }

    pub fn rdates(&self) -> Vec<EventTime> {
        self.comp
            .props_named("RDATE")
            .flat_map(EventTime::parse_property)
            .collect()
    }

    pub fn sequence(&self) -> i64 {
        self.comp
            .value("SEQUENCE")
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0)
    }

    pub fn status(&self) -> Option<String> {
        self.comp
            .value("STATUS")
            .map(|s| s.trim().to_ascii_lowercase())
    }

    pub fn is_cancelled(&self) -> bool {
        self.status().as_deref() == Some("cancelled")
    }

    pub fn is_free(&self) -> bool {
        self.comp
            .value("TRANSP")
            .is_some_and(|t| t.trim().eq_ignore_ascii_case("TRANSPARENT"))
    }

    pub fn alarms(&self) -> Vec<AlarmView> {
        self.comp
            .children()
            .filter(|c| c.name == "VALARM")
            .map(|alarm| {
                let trigger_prop = alarm.prop("TRIGGER");
                let trigger = trigger_prop.and_then(|p| {
                    if p.param("VALUE")
                        .is_some_and(|v| v.eq_ignore_ascii_case("DATE-TIME"))
                    {
                        return match EventTime::parse_first(p)? {
                            EventTime::Utc(ts) => Some(Trigger::At(ts)),
                            _ => None,
                        };
                    }
                    let minutes = IcalDuration::parse(&p.value)?.minutes();
                    if p.param("RELATED")
                        .is_some_and(|r| r.eq_ignore_ascii_case("END"))
                    {
                        Some(Trigger::End(minutes))
                    } else {
                        Some(Trigger::Start(minutes))
                    }
                });
                AlarmView {
                    id: alarm
                        .value("X-WR-ALARMUID")
                        .or_else(|| alarm.value("UID"))
                        .map(|v| v.trim().to_owned()),
                    action: alarm
                        .value("ACTION")
                        .map(|a| a.trim().to_ascii_uppercase())
                        .unwrap_or_default(),
                    trigger,
                    trigger_text: trigger_prop
                        .map(|p| p.value.trim().to_owned())
                        .unwrap_or_default(),
                    acknowledged: alarm.prop("ACKNOWLEDGED").and_then(|p| {
                        match EventTime::parse_first(p)? {
                            EventTime::Utc(ts) => Some(ts),
                            _ => None,
                        }
                    }),
                }
            })
            .collect()
    }

    pub fn organizer(&self) -> Option<Participant> {
        self.comp.prop("ORGANIZER").map(|p| Participant {
            email: mail_address(&p.value),
            name: p.param("CN").map(str::to_owned),
            role: None,
            partstat: None,
        })
    }

    pub fn attendees(&self) -> Vec<Participant> {
        self.comp
            .props_named("ATTENDEE")
            .map(|p| Participant {
                email: mail_address(&p.value),
                name: p.param("CN").map(str::to_owned),
                role: p.param("ROLE").map(str::to_owned),
                partstat: p.param("PARTSTAT").map(str::to_owned),
            })
            .collect()
    }

    /// Scheduling role against the owner's addresses. Unknown owner addresses
    /// with any organizer or attendee read as `attendee` (the protective
    /// choice: such events stay read-only).
    pub fn scheduling_role(&self, owner: &BTreeSet<String>) -> SchedulingRole {
        let is_owner =
            |p: &Participant| p.email.as_ref().is_some_and(|e| owner.contains(e.as_str()));
        let attendees = self.attendees();
        let organizer = self.organizer();
        let others = attendees.iter().filter(|a| !is_owner(a)).count();
        match organizer {
            Some(o) if !is_owner(&o) => SchedulingRole::Attendee,
            Some(_) if others > 0 => SchedulingRole::Organizer,
            None if others > 0 => {
                if owner.is_empty() {
                    SchedulingRole::Attendee
                } else {
                    SchedulingRole::Organizer
                }
            }
            _ => SchedulingRole::None,
        }
    }

    /// The exact span from start to end, in the start's terms: whole days for
    /// a DATE start, otherwise the instant difference.
    pub fn span(&self, zones: &Zones<'_>) -> EventSpan {
        let Some(start) = self.start() else {
            return EventSpan::Exact(SignedDuration::ZERO);
        };
        if let EventTime::Date(d) = &start {
            let days = match (self.dtend(), self.duration()) {
                (Some(EventTime::Date(end)), _) => {
                    end.since(*d).map(|s| i64::from(s.get_days())).unwrap_or(1)
                }
                (_, Some(duration)) => (duration.signed_seconds() / 86_400).max(0),
                _ => 1,
            };
            return EventSpan::Days(days.max(0));
        }
        let exact = match (self.dtend(), self.duration()) {
            (Some(end), _) => match (zones.instant(&start), zones.instant(&end)) {
                (Some(a), Some(b)) => b.duration_since(a),
                _ => SignedDuration::ZERO,
            },
            (None, Some(duration)) => SignedDuration::from_secs(duration.signed_seconds()),
            (None, None) => SignedDuration::ZERO,
        };
        EventSpan::Exact(if exact.is_negative() {
            SignedDuration::ZERO
        } else {
            exact
        })
    }
}

/// The length of an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventSpan {
    Days(i64),
    Exact(SignedDuration),
}

impl EventSpan {
    /// The end value for an occurrence starting at `start`, expressed like
    /// `end_like` (same TZID) when given.
    pub fn end_for(
        &self,
        start: &EventTime,
        end_like: Option<&EventTime>,
        zones: &Zones<'_>,
    ) -> Option<EventTime> {
        match (self, start) {
            (EventSpan::Days(days), EventTime::Date(d)) => {
                d.checked_add(days.days()).ok().map(EventTime::Date)
            }
            (EventSpan::Days(days), other) => {
                let instant = zones.instant(other)?;
                Some(EventTime::Utc(
                    instant
                        .checked_add(SignedDuration::from_hours(24 * days))
                        .ok()?,
                ))
            }
            (EventSpan::Exact(d), _) => {
                let instant = zones.instant(start)?.checked_add(*d).ok()?;
                let reference = end_like.unwrap_or(start);
                Some(match reference {
                    EventTime::Utc(_) => EventTime::Utc(instant),
                    EventTime::Date(_) => EventTime::Utc(instant),
                    other => other.with_civil(zones.civil_in(instant, other)),
                })
            }
        }
    }
}

/// The master VEVENT (no RECURRENCE-ID) of a resource, if any.
pub fn master(doc: &IcsDoc) -> Option<EventView<'_>> {
    doc.events()
        .map(|comp| EventView { comp })
        .find(|e| e.comp.prop("RECURRENCE-ID").is_none())
}

pub fn overrides(doc: &IcsDoc) -> Vec<EventView<'_>> {
    doc.events()
        .map(|comp| EventView { comp })
        .filter(|e| e.comp.prop("RECURRENCE-ID").is_some())
        .collect()
}

/// The first event of the resource (the master, else the first override).
pub fn primary_event(doc: &IcsDoc) -> Option<EventView<'_>> {
    master(doc).or_else(|| doc.events().next().map(|comp| EventView { comp }))
}

/// The inclusive last day of an all-day span.
pub fn last_date(start: Date, end: Option<&EventTime>) -> Date {
    match end {
        Some(EventTime::Date(end)) if *end > start => end.yesterday().unwrap_or(start),
        _ => start,
    }
}

/// The compact projection kept in the mirror and in change rows.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Projection {
    pub summary: Option<String>,
    /// Canonical start (`YYYY-MM-DD`, `...[TZID]`, `...Z` or floating).
    pub start: Option<String>,
    /// Start instant (RFC 3339 UTC).
    pub start_utc: Option<String>,
    pub all_day: bool,
    pub recurring: bool,
    pub time_zone: Option<String>,
    pub has_alarms: bool,
    pub scheduling_role: Option<SchedulingRole>,
    pub location: Option<String>,
    pub status: Option<String>,
}

pub const PROJECTION_TEXT_MAX: usize = 200;

fn clip(text: Option<String>) -> Option<String> {
    text.map(|t| t.chars().take(PROJECTION_TEXT_MAX).collect())
}

pub fn projection(doc: &IcsDoc, zones: &Zones<'_>, owner: &BTreeSet<String>) -> Projection {
    let Some(event) = primary_event(doc) else {
        return Projection::default();
    };
    let start = event.start();
    Projection {
        summary: clip(event.summary()),
        start: start.as_ref().and_then(|s| zones.key(s, s)),
        start_utc: start
            .as_ref()
            .and_then(|s| zones.instant(s))
            .map(super::time::rfc3339),
        all_day: start.as_ref().is_some_and(EventTime::is_date),
        recurring: !event.rrules().is_empty() || !event.rdates().is_empty(),
        time_zone: start.as_ref().and_then(|s| s.tzid().map(str::to_owned)),
        has_alarms: !event.alarms().is_empty(),
        scheduling_role: Some(event.scheduling_role(owner)),
        location: clip(event.location()),
        status: event.status(),
    }
}

/// The semantic fields compared for verification and change detection,
/// keyed by field group; the value is a normalized fingerprint.
pub fn semantic_fields(doc: &IcsDoc) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut events: Vec<EventView<'_>> = doc.events().map(|comp| EventView { comp }).collect();
    events.sort_by_key(|e| {
        e.comp
            .prop("RECURRENCE-ID")
            .map(|p| p.value.clone())
            .unwrap_or_default()
    });
    for event in events {
        let prefix = match event.comp.prop("RECURRENCE-ID") {
            Some(p) => format!("override[{}]", p.value.trim()),
            None => "master".to_owned(),
        };
        let mut put = |group: &str, value: String| out.push((format!("{prefix}.{group}"), value));
        put("uid", event.uid().unwrap_or_default());
        put("title", event.summary().unwrap_or_default());
        let time = |name: &str| {
            event
                .comp
                .prop(name)
                .map(|p| {
                    format!(
                        "{}|{}",
                        p.param("TZID").unwrap_or_default(),
                        EventTime::parse_property(p)
                            .iter()
                            .map(EventTime::value_text)
                            .collect::<Vec<_>>()
                            .join(",")
                    )
                })
                .unwrap_or_default()
        };
        put("start", time("DTSTART"));
        put(
            "end",
            format!(
                "{}|{}",
                time("DTEND"),
                event.comp.value("DURATION").unwrap_or_default().trim()
            ),
        );
        let mut rules: Vec<String> = event
            .rrules()
            .iter()
            .map(|r| {
                RRule::parse(r)
                    .map(|r| r.to_value())
                    .unwrap_or_else(|_| r.clone())
            })
            .collect();
        rules.sort();
        put("recurrence", rules.join("\n"));
        let mut exdates: Vec<String> = event.exdates().iter().map(EventTime::value_text).collect();
        exdates.sort();
        let mut rdates: Vec<String> = event.rdates().iter().map(EventTime::value_text).collect();
        rdates.sort();
        put(
            "exceptions",
            format!("{}|{}", exdates.join(","), rdates.join(",")),
        );
        put("location", event.location().unwrap_or_default());
        put("notes", event.description().unwrap_or_default());
        put("url", event.url().unwrap_or_default());
        put("availability", event.is_free().to_string());
        put("status", event.status().unwrap_or_default());
        let mut alarms: Vec<String> = event
            .alarms()
            .iter()
            .map(|a| format!("{}|{}", a.action, a.trigger_text))
            .collect();
        alarms.sort();
        put("alarms", alarms.join(","));
        let mut people: Vec<String> = event
            .comp
            .props()
            .filter(|p| p.name == "ATTENDEE" || p.name == "ORGANIZER")
            .map(|p| format!("{}:{}", p.name, p.value.trim().to_ascii_lowercase()))
            .collect();
        people.sort();
        put("attendees", people.join(","));
    }
    out
}

/// The change groups that differ between two documents, in a stable order;
/// `other` when only non-semantic content changed.
pub fn changed_fields(before: &IcsDoc, after: &IcsDoc) -> Vec<String> {
    let a = semantic_fields(before);
    let b = semantic_fields(after);
    let groups = [
        "start",
        "end",
        "title",
        "location",
        "notes",
        "url",
        "recurrence",
        "exceptions",
        "alarms",
        "attendees",
        "availability",
        "status",
    ];
    let mut changed: Vec<String> = Vec::new();
    let overrides = |list: &[(String, String)]| -> Vec<String> {
        list.iter()
            .filter(|(k, _)| k.starts_with("override["))
            .map(|(k, v)| format!("{k}={v}"))
            .collect()
    };
    let overrides_changed = overrides(&a) != overrides(&b);
    for group in groups {
        let key = format!("master.{group}");
        let pick = |list: &[(String, String)]| {
            list.iter().find(|(k, _)| *k == key).map(|(_, v)| v.clone())
        };
        if pick(&a) != pick(&b) {
            changed.push(group.to_owned());
        }
    }
    if overrides_changed && !changed.iter().any(|c| c == "exceptions") {
        changed.push("exceptions".to_owned());
    }
    if changed.is_empty() {
        changed.push("other".to_owned());
    }
    changed
}

/// The semantic differences between an intended and a fetched document
/// (verification); empty when they match.
pub fn semantic_mismatches(intended: &IcsDoc, fetched: &IcsDoc) -> Vec<String> {
    let a = semantic_fields(intended);
    let b = semantic_fields(fetched);
    let mut keys: BTreeSet<&String> = a.iter().map(|(k, _)| k).collect();
    keys.extend(b.iter().map(|(k, _)| k));
    keys.into_iter()
        .filter(|k| {
            let va = a.iter().find(|(x, _)| x == *k).map(|(_, v)| v);
            let vb = b.iter().find(|(x, _)| x == *k).map(|(_, v)| v);
            va != vb
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(IcalDuration::parse("-PT15M").unwrap().minutes(), -15);
        assert_eq!(
            IcalDuration::parse("P1DT2H").unwrap().signed_seconds(),
            93_600
        );
        assert_eq!(IcalDuration::parse("P1W").unwrap().days, 7);
        assert!(IcalDuration::parse("PT").is_none());
        assert_eq!(IcalDuration::from_minutes(-90), "-PT90M");
        assert_eq!(IcalDuration::from_minutes(-1440), "-P1D");
        assert_eq!(IcalDuration::from_minutes(540), "PT9H");
    }

    #[test]
    fn scheduling_roles() {
        let doc = IcsDoc::parse(
            "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:a\r\nORGANIZER;CN=Boss:mailto:boss@example.com\r\nATTENDEE:mailto:me@example.com\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        )
        .unwrap();
        let owner: BTreeSet<String> = ["me@example.com".to_owned()].into();
        assert_eq!(
            master(&doc).unwrap().scheduling_role(&owner),
            SchedulingRole::Attendee
        );
        let mine = IcsDoc::parse(
            "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:a\r\nORGANIZER:mailto:ME@example.com\r\nATTENDEE:mailto:me@example.com\r\nATTENDEE:mailto:friend@example.com\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        )
        .unwrap();
        assert_eq!(
            master(&mine).unwrap().scheduling_role(&owner),
            SchedulingRole::Organizer
        );
        let plain = IcsDoc::parse(
            "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:a\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        )
        .unwrap();
        assert_eq!(
            master(&plain).unwrap().scheduling_role(&BTreeSet::new()),
            SchedulingRole::None
        );
    }
}
