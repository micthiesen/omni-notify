//! Apple recurrence service values.
//!
//! Protocol observations, independently implemented from Apple's public web client
//! (Reminders web build 2636Build17): the RecurrenceRule model and its RRule display
//! adapter. Frequency is zero-based; pyicloud's one-based enum must not be used.
//! Selector bounds follow RFC 5545 section 3.3.10. This codec transforms service
//! VALUES only and does not calculate occurrences.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use crate::json::{bounded, integer, js_to_i64};

const MAX_SELECTOR_BYTES: usize = 16_384;
const MAX_FIELD_COUNT: usize = 32;

/// A field that may be absent, explicitly `null`, or set (`optional(NullOr(x))`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Opt<T> {
    #[default]
    Absent,
    Null,
    Set(T),
}

impl<T> Opt<T> {
    pub fn is_absent(&self) -> bool {
        matches!(self, Self::Absent)
    }

    /// The value when set (`x ?? undefined` with null treated as absent).
    pub fn value(&self) -> Option<&T> {
        match self {
            Self::Set(value) => Some(value),
            _ => None,
        }
    }
}

impl<T: Serialize> Serialize for Opt<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Set(value) => value.serialize(serializer),
            Self::Absent | Self::Null => serializer.serialize_none(),
        }
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Opt<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match Option::<T>::deserialize(deserializer)? {
            Some(value) => Self::Set(value),
            None => Self::Null,
        })
    }
}

/// Apple's zero-based frequency order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Frequency {
    Daily,
    Weekly,
    Monthly,
    Yearly,
    Hourly,
    Minutely,
    Secondly,
}

impl Frequency {
    pub const ALL: [Frequency; 7] = [
        Self::Daily,
        Self::Weekly,
        Self::Monthly,
        Self::Yearly,
        Self::Hourly,
        Self::Minutely,
        Self::Secondly,
    ];

    pub fn wire(self) -> i64 {
        match self {
            Self::Daily => 0,
            Self::Weekly => 1,
            Self::Monthly => 2,
            Self::Yearly => 3,
            Self::Hourly => 4,
            Self::Minutely => 5,
            Self::Secondly => 6,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Daily => "daily",
            Self::Weekly => "weekly",
            Self::Monthly => "monthly",
            Self::Yearly => "yearly",
            Self::Hourly => "hourly",
            Self::Minutely => "minutely",
            Self::Secondly => "secondly",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.name() == name)
    }

    /// One occurrence per civil period (daily/weekly/monthly/yearly).
    pub fn is_calendar(self) -> bool {
        matches!(
            self,
            Self::Daily | Self::Weekly | Self::Monthly | Self::Yearly
        )
    }
}

/// `{ dayOfTheWeek, weekNumber? }`; zero `weekNumber` means an unqualified weekday.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DayOfWeek {
    pub day_of_the_week: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub week_number: Option<i64>,
}

/// An explicit rule; missing scalar defaults are never inferred from old clients.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecurrenceRule {
    pub frequency: Frequency,
    pub interval: i64,
    #[serde(default, skip_serializing_if = "Opt::is_absent")]
    pub occurrence_count: Opt<i64>,
    #[serde(default, skip_serializing_if = "Opt::is_absent")]
    pub end_date: Opt<i64>,
    #[serde(default, skip_serializing_if = "Opt::is_absent")]
    pub first_day_of_week: Opt<i64>,
    #[serde(default, skip_serializing_if = "Opt::is_absent")]
    pub days_of_week: Opt<Vec<DayOfWeek>>,
    #[serde(default, skip_serializing_if = "Opt::is_absent")]
    pub days_of_month: Opt<Vec<i64>>,
    #[serde(default, skip_serializing_if = "Opt::is_absent")]
    pub days_of_year: Opt<Vec<i64>>,
    #[serde(default, skip_serializing_if = "Opt::is_absent")]
    pub weeks_of_year: Opt<Vec<i64>>,
    #[serde(default, skip_serializing_if = "Opt::is_absent")]
    pub months_of_year: Opt<Vec<i64>>,
    #[serde(default, skip_serializing_if = "Opt::is_absent")]
    pub set_positions: Opt<Vec<i64>>,
}

impl RecurrenceRule {
    /// Whether any date selector has entries.
    pub fn has_selectors(&self) -> bool {
        self.days_of_week.value().is_some_and(|v| !v.is_empty())
            || [
                &self.days_of_month,
                &self.days_of_year,
                &self.weeks_of_year,
                &self.months_of_year,
                &self.set_positions,
            ]
            .iter()
            .any(|list| list.value().is_some_and(|v| !v.is_empty()))
    }

    /// The rule as its JSON object (schema key order).
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

/// Why a rule is readable but not supported.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnsupportedReason {
    InvalidFields,
    UnknownFields,
    UnknownFrequency,
    InvalidSelector,
    InvalidRule,
}

/// Names only, bounded; never arbitrary persisted payloads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecurrenceUnsupported {
    pub supported: bool,
    pub reason: UnsupportedReason,
    pub fields: Vec<String>,
}

/// `{supported: true, rule}` or the unsupported shape.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecurrenceDecoded {
    Supported(RecurrenceRule),
    Unsupported(RecurrenceUnsupported),
}

impl RecurrenceDecoded {
    pub fn is_supported(&self) -> bool {
        matches!(self, Self::Supported(_))
    }

    pub fn rule(&self) -> Option<&RecurrenceRule> {
        match self {
            Self::Supported(rule) => Some(rule),
            Self::Unsupported(_) => None,
        }
    }
}

impl Serialize for RecurrenceDecoded {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap as _;
        match self {
            Self::Supported(rule) => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("supported", &true)?;
                map.serialize_entry("rule", rule)?;
                map.end()
            }
            Self::Unsupported(unsupported) => unsupported.serialize(serializer),
        }
    }
}

/// Encoded service values, in Apple's field order.
pub type RecurrenceValues = Map<String, Value>;

#[derive(Clone, Debug, PartialEq)]
pub enum RecurrenceEncoded {
    Supported(RecurrenceValues),
    Unsupported(RecurrenceUnsupported),
}

const SCALAR_FIELDS: [(&str, &str); 4] = [
    ("Interval", "interval"),
    ("OccurrenceCount", "occurrenceCount"),
    ("EndDate", "endDate"),
    ("FirstDayOfTheWeek", "firstDayOfWeek"),
];
const SELECTOR_FIELDS: [(&str, &str); 6] = [
    ("DaysOfTheWeek", "daysOfWeek"),
    ("DaysOfTheMonth", "daysOfMonth"),
    ("DaysOfTheYear", "daysOfYear"),
    ("WeeksOfTheYear", "weeksOfYear"),
    ("MonthsOfTheYear", "monthsOfYear"),
    ("SetPositions", "setPositions"),
];

/// Wire names of scalar rule fields (`Frequency` included).
pub(crate) fn is_scalar_wire(name: &str) -> bool {
    name == "Frequency" || SCALAR_FIELDS.iter().any(|(wire, _)| *wire == name)
}

/// Wire names of selector rule fields.
pub(crate) fn is_selector_wire(name: &str) -> bool {
    SELECTOR_FIELDS.iter().any(|(wire, _)| *wire == name)
}

fn unsupported(reason: UnsupportedReason, fields: &[&str]) -> RecurrenceUnsupported {
    RecurrenceUnsupported {
        supported: false,
        reason,
        fields: fields
            .iter()
            .take(MAX_FIELD_COUNT)
            .map(|field| bounded(field, 128))
            .collect(),
    }
}

struct Invalid;

fn int_between(value: &Value, min: f64, max: f64) -> Result<i64, Invalid> {
    integer(value)
        .filter(|n| *n >= min && *n <= max)
        .map(js_to_i64)
        .ok_or(Invalid)
}

fn signed(value: &Value, max: f64) -> Result<i64, Invalid> {
    int_between(value, -max, max).and_then(|n| if n == 0 { Err(Invalid) } else { Ok(n) })
}

/// `optional(NullOr(schema))`.
fn optional<T>(
    object: &Map<String, Value>,
    key: &str,
    decode: impl Fn(&Value) -> Result<T, Invalid>,
) -> Result<Opt<T>, Invalid> {
    match object.get(key) {
        None => Ok(Opt::Absent),
        Some(Value::Null) => Ok(Opt::Null),
        Some(value) => decode(value).map(Opt::Set),
    }
}

fn list(
    value: &Value,
    max_len: usize,
    item: impl Fn(&Value) -> Result<i64, Invalid>,
) -> Result<Vec<i64>, Invalid> {
    let items = value.as_array().ok_or(Invalid)?;
    if items.len() > max_len {
        return Err(Invalid);
    }
    items.iter().map(item).collect()
}

fn day(value: &Value) -> Result<DayOfWeek, Invalid> {
    let object = value.as_object().ok_or(Invalid)?;
    if object
        .keys()
        .any(|key| key != "dayOfTheWeek" && key != "weekNumber")
    {
        return Err(Invalid);
    }
    let day_of_the_week = int_between(object.get("dayOfTheWeek").ok_or(Invalid)?, 1.0, 7.0)?;
    let week_number = match object.get("weekNumber") {
        None => None,
        Some(value) => Some(int_between(value, -53.0, 53.0)?),
    };
    Ok(DayOfWeek {
        day_of_the_week,
        week_number,
    })
}

/// `Schema.decodeUnknownResult(RecurrenceRuleSchema, {onExcessProperty: "error"})`.
fn decode_rule_schema(input: &Value) -> Result<RecurrenceRule, Invalid> {
    const KEYS: [&str; 11] = [
        "frequency",
        "interval",
        "occurrenceCount",
        "endDate",
        "firstDayOfWeek",
        "daysOfWeek",
        "daysOfMonth",
        "daysOfYear",
        "weeksOfYear",
        "monthsOfYear",
        "setPositions",
    ];
    let object = input.as_object().ok_or(Invalid)?;
    if object.keys().any(|key| !KEYS.contains(&key.as_str())) {
        return Err(Invalid);
    }
    let frequency = object
        .get("frequency")
        .and_then(Value::as_str)
        .and_then(Frequency::from_name)
        .ok_or(Invalid)?;
    let interval = int_between(object.get("interval").ok_or(Invalid)?, 1.0, 2_147_483_647.0)?;
    let numeric = |max: f64| move |v: &Value| list(v, 732, |item| signed(item, max));
    Ok(RecurrenceRule {
        frequency,
        interval,
        occurrence_count: optional(object, "occurrenceCount", |v| {
            int_between(v, 0.0, 2_147_483_647.0)
        })?,
        end_date: optional(object, "endDate", |v| {
            int_between(v, 0.0, 253_402_300_799_000.0)
        })?,
        first_day_of_week: optional(object, "firstDayOfWeek", |v| int_between(v, 0.0, 7.0))?,
        days_of_week: optional(object, "daysOfWeek", |v| {
            let items = v.as_array().ok_or(Invalid)?;
            if items.len() > 371 {
                return Err(Invalid);
            }
            items.iter().map(day).collect()
        })?,
        days_of_month: optional(object, "daysOfMonth", numeric(31.0))?,
        days_of_year: optional(object, "daysOfYear", numeric(366.0))?,
        weeks_of_year: optional(object, "weeksOfYear", numeric(53.0))?,
        months_of_year: optional(object, "monthsOfYear", |v| {
            list(v, 732, |item| int_between(item, 1.0, 12.0))
        })?,
        set_positions: optional(object, "setPositions", numeric(366.0))?,
    })
}

fn non_empty<T>(list: &Opt<Vec<T>>) -> bool {
    list.value().is_some_and(|v| !v.is_empty())
}

/// Schema decode plus the RFC 5545 combination checks.
fn validate_rule(input: &Value) -> RecurrenceDecoded {
    let Ok(rule) = decode_rule_schema(input) else {
        return RecurrenceDecoded::Unsupported(unsupported(UnsupportedReason::InvalidRule, &[]));
    };
    let frequency = rule.frequency;
    let qualified_weekday = rule
        .days_of_week
        .value()
        .is_some_and(|days| days.iter().any(|d| d.week_number.is_some_and(|n| n != 0)));
    let other_selector = non_empty(&rule.days_of_week)
        || non_empty(&rule.days_of_month)
        || non_empty(&rule.days_of_year)
        || non_empty(&rule.weeks_of_year)
        || non_empty(&rule.months_of_year);
    let invalid = (rule.occurrence_count.value().copied().unwrap_or(0) > 0
        && rule.end_date.value().is_some())
        || (non_empty(&rule.days_of_month) && frequency == Frequency::Weekly)
        || (non_empty(&rule.days_of_year)
            && matches!(
                frequency,
                Frequency::Daily | Frequency::Weekly | Frequency::Monthly
            ))
        || (non_empty(&rule.weeks_of_year) && frequency != Frequency::Yearly)
        || (qualified_weekday
            && (!matches!(frequency, Frequency::Monthly | Frequency::Yearly)
                || non_empty(&rule.weeks_of_year)))
        || (non_empty(&rule.set_positions) && !other_selector);
    if invalid {
        RecurrenceDecoded::Unsupported(unsupported(UnsupportedReason::InvalidRule, &[]))
    } else {
        RecurrenceDecoded::Supported(rule)
    }
}

fn is_canonical_base64(value: &str) -> bool {
    let bytes = value.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return false;
    }
    let body = |b: &u8| b.is_ascii_alphanumeric() || *b == b'+' || *b == b'/';
    let padding = bytes.iter().rev().take_while(|b| **b == b'=').count();
    padding <= 2 && bytes[..bytes.len() - padding].iter().all(body)
}

/// Supply only recurrence value fields, after separating record identity/link metadata.
pub fn decode_recurrence_values(input: &Value) -> RecurrenceDecoded {
    use base64::Engine as _;
    let Some(values) = input.as_object() else {
        return RecurrenceDecoded::Unsupported(unsupported(UnsupportedReason::InvalidFields, &[]));
    };
    if values.len() > MAX_FIELD_COUNT {
        return RecurrenceDecoded::Unsupported(unsupported(UnsupportedReason::InvalidFields, &[]));
    }
    let unknown: Vec<&str> = values
        .keys()
        .map(String::as_str)
        .filter(|key| !is_scalar_wire(key) && !is_selector_wire(key))
        .collect();
    if !unknown.is_empty() {
        return RecurrenceDecoded::Unsupported(unsupported(
            UnsupportedReason::UnknownFields,
            &unknown,
        ));
    }
    let frequency = values
        .get("Frequency")
        .and_then(integer)
        .filter(|n| *n >= 0.0 && *n < 7.0)
        .and_then(|n| {
            Frequency::ALL
                .get(usize::try_from(js_to_i64(n)).ok()?)
                .copied()
        });
    let Some(frequency) = frequency else {
        return RecurrenceDecoded::Unsupported(unsupported(
            UnsupportedReason::UnknownFrequency,
            &["Frequency"],
        ));
    };
    let mut rule = Map::new();
    rule.insert("frequency".into(), Value::String(frequency.name().into()));
    for (wire, name) in SCALAR_FIELDS {
        if let Some(value) = values.get(wire) {
            rule.insert(name.into(), value.clone());
        }
    }
    for (wire, name) in SELECTOR_FIELDS {
        let Some(value) = values.get(wire) else {
            continue;
        };
        let decoded = match value {
            Value::Null => Some(Value::Null),
            Value::String(text)
                if omni_core::js::utf16_len(text) * 3 <= MAX_SELECTOR_BYTES * 4 + 12
                    && is_canonical_base64(text) =>
            {
                base64::engine::general_purpose::STANDARD
                    .decode(text)
                    .ok()
                    .filter(|bytes| bytes.len() <= MAX_SELECTOR_BYTES)
                    .filter(|bytes| crate::json::base64_encode(bytes) == *text)
                    .and_then(|bytes| {
                        serde_json::from_str::<Value>(&String::from_utf8_lossy(&bytes)).ok()
                    })
            }
            _ => None,
        };
        match decoded {
            Some(parsed) => {
                rule.insert(name.into(), parsed);
            }
            None => {
                return RecurrenceDecoded::Unsupported(unsupported(
                    UnsupportedReason::InvalidSelector,
                    &[wire],
                ));
            }
        }
    }
    validate_rule(&Value::Object(rule))
}

/// Does not choose unverified CloudKit field types or merge away unknown fields.
pub fn encode_recurrence_values(input: &Value) -> RecurrenceEncoded {
    let rule = match validate_rule(input) {
        RecurrenceDecoded::Supported(rule) => rule,
        RecurrenceDecoded::Unsupported(unsupported) => {
            return RecurrenceEncoded::Unsupported(unsupported);
        }
    };
    encode_rule(&rule)
}

/// Encodes an already validated rule.
pub fn encode_rule(rule: &RecurrenceRule) -> RecurrenceEncoded {
    let mut values = Map::new();
    values.insert("Frequency".into(), Value::from(rule.frequency.wire()));
    values.insert("Interval".into(), Value::from(rule.interval));
    let scalars = [
        ("OccurrenceCount", &rule.occurrence_count),
        ("EndDate", &rule.end_date),
        ("FirstDayOfTheWeek", &rule.first_day_of_week),
    ];
    for (wire, value) in scalars {
        match value {
            Opt::Absent => {}
            Opt::Null => {
                values.insert(wire.into(), Value::Null);
            }
            Opt::Set(n) => {
                values.insert(wire.into(), Value::from(*n));
            }
        }
    }
    let selectors: [SelectorJson<'_>; 6] = [
        selector_json("DaysOfTheWeek", &rule.days_of_week),
        selector_json("DaysOfTheMonth", &rule.days_of_month),
        selector_json("DaysOfTheYear", &rule.days_of_year),
        selector_json("WeeksOfTheYear", &rule.weeks_of_year),
        selector_json("MonthsOfTheYear", &rule.months_of_year),
        selector_json("SetPositions", &rule.set_positions),
    ];
    for (wire, json, is_null) in selectors {
        if is_null {
            values.insert(wire.into(), Value::Null);
            continue;
        }
        let Some(json) = json else { continue };
        match json {
            Ok(json) if json.len() <= MAX_SELECTOR_BYTES => {
                values.insert(
                    wire.into(),
                    Value::String(crate::json::base64_encode(json.as_bytes())),
                );
            }
            _ => {
                return RecurrenceEncoded::Unsupported(unsupported(
                    UnsupportedReason::InvalidSelector,
                    &[wire],
                ));
            }
        }
    }
    RecurrenceEncoded::Supported(values)
}

/// `(wire name, JSON text when set, explicitly null)`.
type SelectorJson<'a> = (&'a str, Option<Result<String, serde_json::Error>>, bool);

fn selector_json<'a, T: Serialize>(wire: &'a str, value: &Opt<T>) -> SelectorJson<'a> {
    match value {
        Opt::Absent => (wire, None, false),
        Opt::Null => (wire, None, true),
        Opt::Set(list) => (wire, Some(serde_json::to_string(list)), false),
    }
}

#[cfg(test)]
mod recurrence_spec {
    //! Port of `src/reminders/recurrence.spec.ts`.
    //!
    //! Dropped inputs: `endDate: Infinity` and `Frequency: undefined` cannot be
    //! expressed in JSON; the range checks and the missing-`Frequency` case cover them.
    use super::*;
    use serde_json::json;

    fn bytes(value: Value) -> String {
        crate::json::base64_encode(serde_json::to_string(&value).unwrap().as_bytes())
    }

    fn supported_values(encoded: RecurrenceEncoded) -> Map<String, Value> {
        match encoded {
            RecurrenceEncoded::Supported(values) => values,
            RecurrenceEncoded::Unsupported(u) => panic!("unsupported: {u:?}"),
        }
    }

    #[test]
    fn round_trips_each_frequency_with_apples_zero_based_frequency() {
        for (index, frequency) in Frequency::ALL.iter().enumerate() {
            let encoded =
                encode_recurrence_values(&json!({"frequency": frequency.name(), "interval": 1}));
            let values = supported_values(encoded);
            assert_eq!(
                Value::Object(values.clone()),
                json!({"Frequency": index, "Interval": 1})
            );
            let decoded = decode_recurrence_values(&Value::Object(values));
            assert_eq!(
                serde_json::to_value(&decoded).unwrap(),
                json!({"supported": true, "rule": {"frequency": frequency.name(), "interval": 1}})
            );
        }
    }

    #[test]
    fn round_trips_every_selector_without_changing_signed_values_absent_fields_nulls_or_weekday_ordinals()
     {
        let values = json!({
            "Frequency": 3,
            "Interval": 2,
            "OccurrenceCount": 0,
            "FirstDayOfTheWeek": 2,
            "EndDate": 1_800_000_000_000_i64,
            "DaysOfTheWeek": bytes(json!([{"dayOfTheWeek": 2, "weekNumber": 0}, {"dayOfTheWeek": 7}])),
            "DaysOfTheMonth": bytes(json!([1, -1])),
            "DaysOfTheYear": bytes(json!([1, -366])),
            "WeeksOfTheYear": bytes(json!([1, -53])),
            "MonthsOfTheYear": bytes(json!([1, 12])),
            "SetPositions": bytes(json!([1, -1])),
        });
        let decoded = decode_recurrence_values(&values);
        let rule = decoded.rule().expect("supported");
        let encoded = supported_values(encode_rule(rule));
        // toEqual ignores key order.
        let mut expected = values.as_object().unwrap().clone();
        expected.sort_keys();
        let mut actual = encoded.clone();
        actual.sort_keys();
        assert_eq!(actual, expected);
        assert_eq!(
            serde_json::to_value(decode_recurrence_values(
                &json!({"Frequency": 0, "Interval": 1, "DaysOfTheWeek": null})
            ))
            .unwrap(),
            json!({"supported": true, "rule": {"frequency": "daily", "interval": 1, "daysOfWeek": null}})
        );
        assert_eq!(
            Value::Object(supported_values(encode_recurrence_values(&json!({
                "frequency": "monthly",
                "interval": 1,
                "daysOfWeek": [{"dayOfTheWeek": 2, "weekNumber": -1}],
            })))),
            json!({
                "Frequency": 2,
                "Interval": 1,
                "DaysOfTheWeek": bytes(json!([{"dayOfTheWeek": 2, "weekNumber": -1}])),
            })
        );
    }

    #[test]
    fn keeps_unknown_frequency_unsupported_without_a_daily_fallback() {
        for frequency in [json!(-1), json!(7), json!(1.5), json!("0"), json!(null)] {
            let decoded = decode_recurrence_values(&json!({"Frequency": frequency, "Interval": 1}));
            match decoded {
                RecurrenceDecoded::Unsupported(u) => {
                    assert_eq!(u.reason, UnsupportedReason::UnknownFrequency);
                }
                RecurrenceDecoded::Supported(_) => panic!("{frequency} decoded"),
            }
        }
        let missing = decode_recurrence_values(&json!({"Interval": 1}));
        assert!(matches!(
            missing,
            RecurrenceDecoded::Unsupported(RecurrenceUnsupported {
                reason: UnsupportedReason::UnknownFrequency,
                ..
            })
        ));
    }

    #[test]
    fn does_not_infer_absent_defaults_or_silently_discard_unknown_fields() {
        assert!(!decode_recurrence_values(&json!({"Frequency": 0})).is_supported());
        assert_eq!(
            serde_json::to_value(decode_recurrence_values(
                &json!({"Frequency": 0, "Interval": 1, "NewRule": "private-data"})
            ))
            .unwrap(),
            json!({"supported": false, "reason": "unknown_fields", "fields": ["NewRule"]})
        );
        assert!(matches!(
            encode_recurrence_values(
                &json!({"frequency": "daily", "interval": 1, "newRule": true})
            ),
            RecurrenceEncoded::Unsupported(_)
        ));
        assert!(
            !decode_recurrence_values(&json!({
                "Frequency": 0,
                "Interval": 1,
                "DaysOfTheWeek": bytes(json!([{"dayOfTheWeek": 2, "unknown": true}])),
            }))
            .is_supported()
        );
    }

    #[test]
    fn rejects_malformed_or_oversized_selector_without_exposing_it() {
        let cases = [
            "%%%".to_owned(),
            "W10".to_owned(),
            "W11=".to_owned(),
            bytes(json!("not an array")),
            bytes(json!({})),
            crate::json::base64_encode(b"not json"),
            "A".repeat(30_000),
        ];
        for selector in cases {
            let result = decode_recurrence_values(
                &json!({"Frequency": 0, "Interval": 1, "DaysOfTheWeek": selector}),
            );
            assert!(!result.is_supported());
            assert!(
                !serde_json::to_string(&result)
                    .unwrap()
                    .contains(selector.as_str())
            );
        }
    }

    #[test]
    fn rejects_out_of_range_values() {
        let patches = [
            json!({"interval": 0}),
            json!({"interval": 1.5}),
            json!({"occurrenceCount": -1}),
            json!({"endDate": -1}),
            json!({"endDate": 253_402_300_799_001_i64}),
            json!({"firstDayOfWeek": -1}),
            json!({"firstDayOfWeek": 8}),
            json!({"daysOfWeek": [{"dayOfTheWeek": 0}]}),
            json!({"daysOfWeek": [{"dayOfTheWeek": 8}]}),
            json!({"daysOfWeek": [{"dayOfTheWeek": 2, "weekNumber": 54}]}),
            json!({"daysOfMonth": [0]}),
            json!({"daysOfMonth": [32]}),
            json!({"daysOfYear": [-367]}),
            json!({"weeksOfYear": [54]}),
            json!({"monthsOfYear": [13]}),
            json!({"setPositions": [0]}),
        ];
        for patch in patches {
            let mut input = json!({"frequency": "yearly", "interval": 1});
            input
                .as_object_mut()
                .unwrap()
                .extend(patch.as_object().unwrap().clone());
            assert!(
                matches!(
                    encode_recurrence_values(&input),
                    RecurrenceEncoded::Unsupported(_)
                ),
                "{patch}"
            );
        }
    }

    #[test]
    fn rejects_incompatible_selectors() {
        let patches = [
            json!({"frequency": "weekly", "daysOfMonth": [1]}),
            json!({"frequency": "monthly", "daysOfYear": [1]}),
            json!({"frequency": "daily", "weeksOfYear": [1]}),
            json!({"frequency": "weekly", "daysOfWeek": [{"dayOfTheWeek": 2, "weekNumber": 1}]}),
            json!({"frequency": "yearly", "weeksOfYear": [1], "daysOfWeek": [{"dayOfTheWeek": 2, "weekNumber": 1}]}),
            json!({"setPositions": [1]}),
            json!({"occurrenceCount": 3, "endDate": 1_800_000_000_000_i64}),
        ];
        for patch in patches {
            let mut input = json!({"frequency": "daily", "interval": 1});
            input
                .as_object_mut()
                .unwrap()
                .extend(patch.as_object().unwrap().clone());
            assert!(
                matches!(
                    encode_recurrence_values(&input),
                    RecurrenceEncoded::Unsupported(_)
                ),
                "{patch}"
            );
        }
    }

    #[test]
    fn handles_non_object_inputs_without_throwing() {
        for value in [
            json!(null),
            json!(4),
            json!("rule"),
            json!([]),
            json!(false),
        ] {
            assert!(!decode_recurrence_values(&value).is_supported());
            assert!(matches!(
                encode_recurrence_values(&value),
                RecurrenceEncoded::Unsupported(_)
            ));
        }
    }

    #[test]
    fn preserves_the_observed_server_first_day_value_zero_without_interpreting_it_as_sunday() {
        let values = json!({"Frequency": 0, "Interval": 1, "FirstDayOfTheWeek": 0});
        let decoded = decode_recurrence_values(&values);
        assert_eq!(
            serde_json::to_value(&decoded).unwrap(),
            json!({"supported": true, "rule": {"frequency": "daily", "interval": 1, "firstDayOfWeek": 0}})
        );
        let encoded = supported_values(encode_rule(decoded.rule().unwrap()));
        assert_eq!(Value::Object(encoded), values);
    }
}
