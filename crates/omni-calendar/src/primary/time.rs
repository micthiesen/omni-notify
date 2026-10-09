//! Event times: DATE / zoned / UTC / floating values, time-zone resolution
//! (jiff is the only source of UTC offsets), canonical recurrence IDs, and
//! VTIMEZONE generation and evaluation.

use jiff::civil::{Date, DateTime, Time};
use jiff::tz::{AmbiguousOffset, Offset, TimeZone};
use jiff::{SignedDuration, Timestamp};

use super::ics::{Component, IcsDoc, Property};
use super::rrule::RRule;

/// One iCalendar date or date-time value.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum EventTime {
    /// `VALUE=DATE`.
    Date(Date),
    /// `TZID=<tzid>:<local>`.
    Zoned { local: DateTime, tzid: String },
    /// `...Z`.
    Utc(Timestamp),
    /// No zone.
    Floating(DateTime),
}

impl EventTime {
    pub fn is_date(&self) -> bool {
        matches!(self, EventTime::Date(_))
    }

    pub fn tzid(&self) -> Option<&str> {
        match self {
            EventTime::Zoned { tzid, .. } => Some(tzid),
            _ => None,
        }
    }

    /// The civil value (midnight for a date; UTC civil for a UTC value).
    pub fn civil(&self) -> DateTime {
        match self {
            EventTime::Date(d) => d.to_datetime(Time::midnight()),
            EventTime::Zoned { local, .. } | EventTime::Floating(local) => *local,
            EventTime::Utc(ts) => ts.to_zoned(TimeZone::UTC).datetime(),
        }
    }

    /// The same kind and zone with another civil value.
    pub fn with_civil(&self, civil: DateTime) -> EventTime {
        match self {
            EventTime::Date(_) => EventTime::Date(civil.date()),
            EventTime::Zoned { tzid, .. } => EventTime::Zoned {
                local: civil,
                tzid: tzid.clone(),
            },
            EventTime::Floating(_) => EventTime::Floating(civil),
            EventTime::Utc(_) => civil
                .to_zoned(TimeZone::UTC)
                .map(|z| EventTime::Utc(z.timestamp()))
                .unwrap_or(EventTime::Floating(civil)),
        }
    }

    /// Parses every value of a DTSTART/DTEND/EXDATE/RDATE/RECURRENCE-ID
    /// property (comma-separated lists allowed). PERIOD values are skipped.
    pub fn parse_property(property: &Property) -> Vec<EventTime> {
        let is_date = property
            .param("VALUE")
            .is_some_and(|v| v.eq_ignore_ascii_case("DATE"));
        if property
            .param("VALUE")
            .is_some_and(|v| v.eq_ignore_ascii_case("PERIOD"))
        {
            return Vec::new();
        }
        let tzid = property.param("TZID");
        property
            .value
            .split(',')
            .filter_map(|v| parse_value(v.trim(), is_date, tzid))
            .collect()
    }

    pub fn parse_first(property: &Property) -> Option<EventTime> {
        Self::parse_property(property).into_iter().next()
    }

    /// The property `name` carrying this value.
    pub fn to_property(&self, name: &str) -> Property {
        match self {
            EventTime::Date(d) => Property::new(name, format_date(*d)).with_param("VALUE", "DATE"),
            EventTime::Zoned { local, tzid } => {
                Property::new(name, format_local(*local)).with_param("TZID", tzid)
            }
            EventTime::Utc(ts) => Property::new(name, format_utc(*ts)),
            EventTime::Floating(local) => Property::new(name, format_local(*local)),
        }
    }

    /// The raw value text (no parameters).
    pub fn value_text(&self) -> String {
        match self {
            EventTime::Date(d) => format_date(*d),
            EventTime::Zoned { local, .. } | EventTime::Floating(local) => format_local(*local),
            EventTime::Utc(ts) => format_utc(*ts),
        }
    }
}

fn parse_value(value: &str, is_date: bool, tzid: Option<&str>) -> Option<EventTime> {
    if is_date || (value.len() == 8 && value.bytes().all(|b| b.is_ascii_digit())) {
        return parse_date(value).map(EventTime::Date);
    }
    let (body, utc) = match value.strip_suffix(['Z', 'z']) {
        Some(body) => (body, true),
        None => (value, false),
    };
    let local = parse_local(body)?;
    if utc {
        return local
            .to_zoned(TimeZone::UTC)
            .ok()
            .map(|z| EventTime::Utc(z.timestamp()));
    }
    match tzid {
        Some(tzid) if !tzid.is_empty() => Some(EventTime::Zoned {
            local,
            tzid: tzid.to_owned(),
        }),
        _ => Some(EventTime::Floating(local)),
    }
}

/// `YYYYMMDD`.
pub fn parse_date(value: &str) -> Option<Date> {
    if value.len() != 8 || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let year: i16 = value.get(0..4)?.parse().ok()?;
    let month: i8 = value.get(4..6)?.parse().ok()?;
    let day: i8 = value.get(6..8)?.parse().ok()?;
    Date::new(year, month, day).ok()
}

/// `YYYYMMDDTHHMMSS` (seconds optional for leniency).
pub fn parse_local(value: &str) -> Option<DateTime> {
    let (date, time) = value.split_once(['T', 't'])?;
    let date = parse_date(date)?;
    if !(time.len() == 6 || time.len() == 4) || !time.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let hour: i8 = time.get(0..2)?.parse().ok()?;
    let minute: i8 = time.get(2..4)?.parse().ok()?;
    let second: i8 = time.get(4..6).map_or(Some(0), |s| s.parse().ok())?;
    // A leap second reads as :59.
    let time = Time::new(hour, minute, second.min(59), 0).ok()?;
    Some(date.to_datetime(time))
}

pub fn format_date(d: Date) -> String {
    d.strftime("%Y%m%d").to_string()
}

pub fn format_local(dt: DateTime) -> String {
    dt.strftime("%Y%m%dT%H%M%S").to_string()
}

pub fn format_utc(ts: Timestamp) -> String {
    ts.strftime("%Y%m%dT%H%M%SZ").to_string()
}

/// RFC 3339 UTC with seconds.
pub fn rfc3339(ts: Timestamp) -> String {
    ts.strftime("%Y-%m-%dT%H:%M:%SZ").to_string()
}

pub fn iso_date(d: Date) -> String {
    d.strftime("%Y-%m-%d").to_string()
}

pub fn iso_local(dt: DateTime) -> String {
    dt.strftime("%Y-%m-%dT%H:%M:%S").to_string()
}

/// Windows zone names Outlook and Exchange write as TZIDs.
const WINDOWS_ZONES: &[(&str, &str)] = &[
    ("Pacific Standard Time", "America/Los_Angeles"),
    ("Mountain Standard Time", "America/Denver"),
    ("US Mountain Standard Time", "America/Phoenix"),
    ("Central Standard Time", "America/Chicago"),
    ("Eastern Standard Time", "America/New_York"),
    ("Atlantic Standard Time", "America/Halifax"),
    ("Newfoundland Standard Time", "America/St_Johns"),
    ("Alaskan Standard Time", "America/Anchorage"),
    ("Hawaiian Standard Time", "Pacific/Honolulu"),
    ("GMT Standard Time", "Europe/London"),
    ("Greenwich Standard Time", "Atlantic/Reykjavik"),
    ("W. Europe Standard Time", "Europe/Berlin"),
    ("Romance Standard Time", "Europe/Paris"),
    ("Central Europe Standard Time", "Europe/Budapest"),
    ("Central European Standard Time", "Europe/Warsaw"),
    ("E. Europe Standard Time", "Europe/Chisinau"),
    ("FLE Standard Time", "Europe/Kiev"),
    ("Tokyo Standard Time", "Asia/Tokyo"),
    ("China Standard Time", "Asia/Shanghai"),
    ("India Standard Time", "Asia/Kolkata"),
    ("AUS Eastern Standard Time", "Australia/Sydney"),
    ("New Zealand Standard Time", "Pacific/Auckland"),
    ("UTC", "UTC"),
    ("Coordinated Universal Time", "UTC"),
];

/// A TZID resolved to an IANA zone: the zone itself, a Windows name, a
/// vendor-prefixed path (`/mozilla.org/.../America/Vancouver`), or a
/// VTIMEZONE's `X-LIC-LOCATION`.
pub fn resolve_tzid(tzid: &str, vtimezone: Option<&Component>) -> Option<(String, TimeZone)> {
    let trimmed = tzid.trim().trim_matches('"');
    if let Ok(tz) = TimeZone::get(trimmed) {
        return Some((tz.iana_name().unwrap_or(trimmed).to_owned(), tz));
    }
    if let Some((_, iana)) = WINDOWS_ZONES
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(trimmed))
        && let Ok(tz) = TimeZone::get(iana)
    {
        return Some(((*iana).to_owned(), tz));
    }
    let segments: Vec<&str> = trimmed.split('/').filter(|s| !s.is_empty()).collect();
    for start in 1..segments.len() {
        let candidate = segments[start..].join("/");
        if let Ok(tz) = TimeZone::get(&candidate) {
            return Some((candidate, tz));
        }
    }
    let location = vtimezone.and_then(|c| c.value("X-LIC-LOCATION"))?;
    TimeZone::get(location.trim())
        .ok()
        .map(|tz| (location.trim().to_owned(), tz))
}

/// Resolves times inside one document: IANA zones through jiff, otherwise the
/// document's own VTIMEZONE, otherwise the default zone.
pub struct Zones<'a> {
    doc: &'a IcsDoc,
    pub default_tz: TimeZone,
}

impl<'a> Zones<'a> {
    pub fn new(doc: &'a IcsDoc, default_tz: TimeZone) -> Self {
        Self { doc, default_tz }
    }

    pub fn iana(&self, tzid: &str) -> Option<(String, TimeZone)> {
        resolve_tzid(tzid, self.doc.timezone(tzid))
    }

    /// The instant of a value; a date is local midnight in the default zone and
    /// a floating time is read in the default zone.
    pub fn instant(&self, time: &EventTime) -> Option<Timestamp> {
        match time {
            EventTime::Utc(ts) => Some(*ts),
            EventTime::Date(d) => compatible(&self.default_tz, d.to_datetime(Time::midnight())),
            EventTime::Floating(local) => compatible(&self.default_tz, *local),
            EventTime::Zoned { local, tzid } => match self.iana(tzid) {
                Some((_, tz)) => compatible(&tz, *local),
                None => match self
                    .doc
                    .timezone(tzid)
                    .and_then(|vt| vtimezone_offset(vt, *local))
                {
                    Some(offset) => local
                        .to_zoned(TimeZone::fixed(offset))
                        .ok()
                        .map(|z| z.timestamp()),
                    None => compatible(&self.default_tz, *local),
                },
            },
        }
    }

    /// The civil value of an instant in `reference`'s zone (used to key
    /// EXDATE/RECURRENCE-ID values written in another zone).
    pub fn civil_in(&self, instant: Timestamp, reference: &EventTime) -> DateTime {
        match reference {
            EventTime::Zoned { tzid, .. } => match self.iana(tzid) {
                Some((_, tz)) => instant.to_zoned(tz).datetime(),
                None => instant.to_zoned(self.default_tz.clone()).datetime(),
            },
            EventTime::Utc(_) => instant.to_zoned(TimeZone::UTC).datetime(),
            EventTime::Date(_) | EventTime::Floating(_) => {
                instant.to_zoned(self.default_tz.clone()).datetime()
            }
        }
    }

    /// Whether the document's own VTIMEZONE gives a different UTC offset than
    /// jiff for a zoned local time.
    pub fn rules_differ(&self, time: &EventTime) -> bool {
        let EventTime::Zoned { local, tzid } = time else {
            return false;
        };
        let (Some((_, tz)), Some(vt)) = (self.iana(tzid), self.doc.timezone(tzid)) else {
            return false;
        };
        let Some(theirs) = vtimezone_offset(vt, *local) else {
            return false;
        };
        match tz.to_ambiguous_timestamp(*local).offset() {
            AmbiguousOffset::Unambiguous { offset } => offset != theirs,
            AmbiguousOffset::Gap { before, after } | AmbiguousOffset::Fold { before, after } => {
                before != theirs && after != theirs
            }
        }
    }

    /// The canonical occurrence key of `time` in the zone and kind of a series
    /// starting at `master`.
    pub fn key(&self, time: &EventTime, master: &EventTime) -> Option<String> {
        let civil = self.to_master_civil(time, master)?;
        Some(format_key(master, civil))
    }

    /// `time` expressed as a civil value in the master's zone and kind.
    pub fn to_master_civil(&self, time: &EventTime, master: &EventTime) -> Option<DateTime> {
        Some(match (master, time) {
            (EventTime::Date(_), t) => t.civil().date().to_datetime(Time::midnight()),
            (EventTime::Floating(_), EventTime::Utc(ts)) => {
                ts.to_zoned(self.default_tz.clone()).datetime()
            }
            (EventTime::Floating(_), t) => t.civil(),
            (EventTime::Zoned { tzid: m, .. }, EventTime::Zoned { local, tzid }) if m == tzid => {
                *local
            }
            (EventTime::Zoned { .. }, EventTime::Floating(local)) => *local,
            (EventTime::Zoned { .. }, EventTime::Date(d)) => d.to_datetime(master.civil().time()),
            (_, t) => self.civil_in(self.instant(t)?, master),
        })
    }
}

/// `TimeZone::to_ambiguous_zoned(..).compatible()`: a gap moves forward, a
/// fold takes the earlier instant.
pub fn compatible(tz: &TimeZone, local: DateTime) -> Option<Timestamp> {
    tz.to_ambiguous_timestamp(local).compatible().ok()
}

/// Whether a local time falls into a DST gap in `tz`.
pub fn is_nonexistent(tz: &TimeZone, local: DateTime) -> bool {
    matches!(
        tz.to_ambiguous_timestamp(local).offset(),
        AmbiguousOffset::Gap { .. }
    )
}

/// The canonical key: `YYYY-MM-DD` (all-day), `...T..:..:..[TZID]` (zoned),
/// `...Z` (UTC), or bare local (floating).
pub fn format_key(master: &EventTime, civil: DateTime) -> String {
    match master {
        EventTime::Date(_) => iso_date(civil.date()),
        EventTime::Zoned { tzid, .. } => format!("{}[{tzid}]", iso_local(civil)),
        EventTime::Utc(_) => format!("{}Z", iso_local(civil)),
        EventTime::Floating(_) => iso_local(civil),
    }
}

/// Parses a canonical key back into a value (the zone of a `[..]` suffix is
/// kept as written).
pub fn parse_key(key: &str) -> Option<EventTime> {
    let key = key.trim();
    if key.len() == 10 {
        return key.parse::<Date>().ok().map(EventTime::Date);
    }
    if let Some((local, rest)) = key.split_once('[') {
        let tzid = rest.strip_suffix(']')?;
        return Some(EventTime::Zoned {
            local: local.parse::<DateTime>().ok()?,
            tzid: tzid.to_owned(),
        });
    }
    if let Some(local) = key.strip_suffix('Z') {
        let dt = local.parse::<DateTime>().ok()?;
        return dt
            .to_zoned(TimeZone::UTC)
            .ok()
            .map(|z| EventTime::Utc(z.timestamp()));
    }
    key.parse::<DateTime>().ok().map(EventTime::Floating)
}

/// `+HHMM[SS]` / `-HHMM[SS]`.
pub fn parse_utc_offset(value: &str) -> Option<Offset> {
    let value = value.trim();
    let (sign, digits) = match value.as_bytes().first()? {
        b'+' => (1, &value[1..]),
        b'-' => (-1, &value[1..]),
        _ => return None,
    };
    if !(digits.len() == 4 || digits.len() == 6) || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let hours: i32 = digits.get(0..2)?.parse().ok()?;
    let minutes: i32 = digits.get(2..4)?.parse().ok()?;
    let seconds: i32 = digits.get(4..6).map_or(Some(0), |s| s.parse().ok())?;
    Offset::from_seconds(sign * (hours * 3600 + minutes * 60 + seconds)).ok()
}

fn format_utc_offset(offset: Offset) -> String {
    let total = offset.seconds();
    let sign = if total < 0 { '-' } else { '+' };
    let abs = total.abs();
    let (h, m, s) = (abs / 3600, (abs % 3600) / 60, abs % 60);
    if s == 0 {
        format!("{sign}{h:02}{m:02}")
    } else {
        format!("{sign}{h:02}{m:02}{s:02}")
    }
}

/// The offset a VTIMEZONE assigns to a local time: the latest observance
/// onset at or before it (onsets expanded from DTSTART, RDATE and RRULE).
pub fn vtimezone_offset(vtimezone: &Component, local: DateTime) -> Option<Offset> {
    let mut best: Option<(DateTime, Offset)> = None;
    let mut earliest: Option<(DateTime, Offset)> = None;
    for observance in vtimezone
        .children()
        .filter(|c| c.name == "STANDARD" || c.name == "DAYLIGHT")
    {
        let Some(start) = observance
            .prop("DTSTART")
            .and_then(EventTime::parse_first)
            .map(|t| t.civil())
        else {
            continue;
        };
        let Some(to) = observance.value("TZOFFSETTO").and_then(parse_utc_offset) else {
            continue;
        };
        let from = observance
            .value("TZOFFSETFROM")
            .and_then(parse_utc_offset)
            .unwrap_or(to);
        if earliest.is_none_or(|(at, _)| start < at) {
            earliest = Some((start, from));
        }
        let mut onsets = vec![start];
        for rdate in observance.props_named("RDATE") {
            onsets.extend(
                EventTime::parse_property(rdate)
                    .iter()
                    .map(EventTime::civil),
            );
        }
        if let Some(rule) = observance.value("RRULE").and_then(|v| RRule::parse(v).ok()) {
            let expansion = rule.expand(start, None, local, 2_000, |dt| {
                rule.until_allows(dt, |d| {
                    Some(d.to_zoned(TimeZone::fixed(from)).ok()?.timestamp())
                })
            });
            onsets.extend(expansion.instances);
        }
        for onset in onsets.into_iter().filter(|o| *o <= local) {
            if best.is_none_or(|(at, _)| onset > at) {
                best = Some((onset, to));
            }
        }
    }
    best.or(earliest).map(|(_, offset)| offset)
}

/// A VTIMEZONE for an IANA zone generated from jiff's transitions between one
/// year before `from` and `until` (explicit observances, no RRULEs). A zone
/// without transitions in that span gets one STANDARD block.
pub fn generate_vtimezone(
    iana: &str,
    tz: &TimeZone,
    from: Timestamp,
    until: Timestamp,
) -> Component {
    let mut component = Component::new("VTIMEZONE");
    component.insert_prop(Property::new("TZID", iana));
    let start = from
        .checked_sub(SignedDuration::from_hours(24 * 366))
        .unwrap_or(from);
    let mut observances: Vec<Component> = Vec::new();
    // The observance in effect at the start of the span.
    if let Some(previous) = tz.preceding(start).next() {
        let before = tz.to_offset(
            previous
                .timestamp()
                .checked_sub(SignedDuration::from_secs(1))
                .unwrap_or(previous.timestamp()),
        );
        observances.push(observance(
            previous.timestamp(),
            before,
            previous.offset(),
            previous.abbreviation(),
            previous.dst().is_dst(),
        ));
    }
    for transition in tz.following(start) {
        if transition.timestamp() > until || observances.len() >= 400 {
            break;
        }
        let before = tz.to_offset(
            transition
                .timestamp()
                .checked_sub(SignedDuration::from_secs(1))
                .unwrap_or(transition.timestamp()),
        );
        observances.push(observance(
            transition.timestamp(),
            before,
            transition.offset(),
            transition.abbreviation(),
            transition.dst().is_dst(),
        ));
    }
    if observances.is_empty() {
        let info = tz.to_offset_info(from);
        let mut standard = Component::new("STANDARD");
        standard.insert_prop(Property::new("DTSTART", "19700101T000000"));
        standard.insert_prop(Property::new(
            "TZOFFSETFROM",
            format_utc_offset(info.offset()),
        ));
        standard.insert_prop(Property::new(
            "TZOFFSETTO",
            format_utc_offset(info.offset()),
        ));
        standard.insert_prop(Property::new("TZNAME", info.abbreviation()));
        observances.push(standard);
    }
    for o in observances {
        component.push_child(o);
    }
    component
}

fn observance(at: Timestamp, from: Offset, to: Offset, name: &str, dst: bool) -> Component {
    let mut c = Component::new(if dst { "DAYLIGHT" } else { "STANDARD" });
    let local = at.to_zoned(TimeZone::fixed(from)).datetime();
    c.insert_prop(Property::new("DTSTART", format_local(local)));
    c.insert_prop(Property::new("TZOFFSETFROM", format_utc_offset(from)));
    c.insert_prop(Property::new("TZOFFSETTO", format_utc_offset(to)));
    if !name.is_empty() {
        c.insert_prop(Property::new("TZNAME", name));
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt(s: &str) -> DateTime {
        s.parse().unwrap()
    }

    #[test]
    fn parses_value_kinds() {
        let p = Property::new("DTSTART", "20261102T100000").with_param("TZID", "America/Vancouver");
        assert_eq!(
            EventTime::parse_first(&p),
            Some(EventTime::Zoned {
                local: dt("2026-11-02T10:00"),
                tzid: "America/Vancouver".into()
            })
        );
        let d = Property::new("DTSTART", "20261102").with_param("VALUE", "DATE");
        assert_eq!(
            EventTime::parse_first(&d),
            Some(EventTime::Date("2026-11-02".parse().unwrap()))
        );
        let ex = Property::new("EXDATE", "20261102T180000Z,20261109T180000Z");
        assert_eq!(EventTime::parse_property(&ex).len(), 2);
    }

    #[test]
    fn keys_round_trip_and_convert_zones() {
        let doc = IcsDoc::parse("BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n").unwrap();
        let zones = Zones::new(&doc, TimeZone::get("America/Vancouver").unwrap());
        let master = EventTime::Zoned {
            local: dt("2026-07-06T09:00"),
            tzid: "America/Vancouver".into(),
        };
        let utc = EventTime::Utc("2026-07-13T16:00:00Z".parse().unwrap());
        assert_eq!(
            zones.key(&utc, &master).as_deref(),
            Some("2026-07-13T09:00:00[America/Vancouver]")
        );
        let parsed = parse_key("2026-07-13T09:00:00[America/Vancouver]").unwrap();
        assert_eq!(
            zones.key(&parsed, &master).as_deref(),
            Some("2026-07-13T09:00:00[America/Vancouver]")
        );
    }

    #[test]
    fn resolves_windows_and_prefixed_tzids() {
        assert_eq!(
            resolve_tzid("Pacific Standard Time", None).map(|(n, _)| n),
            Some("America/Los_Angeles".to_owned())
        );
        assert_eq!(
            resolve_tzid("/mozilla.org/20050126_1/America/Vancouver", None).map(|(n, _)| n),
            Some("America/Vancouver".to_owned())
        );
        assert!(resolve_tzid("Mars/Olympus", None).is_none());
    }

    #[test]
    fn detects_gaps() {
        let tz = TimeZone::get("America/New_York").unwrap();
        assert!(is_nonexistent(&tz, dt("2026-03-08T02:30")));
        assert!(!is_nonexistent(&tz, dt("2026-03-08T03:30")));
    }

    #[test]
    fn generated_vtimezone_matches_jiff() {
        let tz = TimeZone::get("America/New_York").unwrap();
        let from: Timestamp = "2026-01-01T00:00:00Z".parse().unwrap();
        let until: Timestamp = "2028-01-01T00:00:00Z".parse().unwrap();
        let vt = generate_vtimezone("America/New_York", &tz, from, until);
        for local in ["2026-07-01T12:00", "2026-12-01T12:00", "2027-03-14T12:00"] {
            let local = dt(local);
            let expected = tz.to_ambiguous_timestamp(local).compatible().unwrap();
            let offset = vtimezone_offset(&vt, local).unwrap();
            assert_eq!(
                local.to_zoned(TimeZone::fixed(offset)).unwrap().timestamp(),
                expected,
                "{local}"
            );
        }
    }

    #[test]
    fn vancouver_keeps_permanent_time_from_november_2026() {
        let tz = TimeZone::get("America/Vancouver").unwrap();
        let summer = compatible(&tz, dt("2026-07-01T10:00")).unwrap();
        let winter = compatible(&tz, dt("2026-12-01T10:00")).unwrap();
        assert_eq!(summer.to_string(), "2026-07-01T17:00:00Z");
        assert_eq!(winter.to_string(), "2026-12-01T17:00:00Z");
    }
}
