//! A JSON property that keeps JS `undefined` (absent) apart from `null`.
//!
//! Observer issue revisions hash `JSON.stringify` output, which omits
//! undefined properties but writes `null`, so the distinction is load-bearing.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Absent, `null`, or a value. Use with
/// `#[serde(default, skip_serializing_if = "JsonOpt::is_absent")]`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum JsonOpt<T> {
    #[default]
    Absent,
    Null,
    Value(T),
}

impl<T> JsonOpt<T> {
    pub fn is_absent(&self) -> bool {
        matches!(self, JsonOpt::Absent)
    }

    /// The value, treating absent and `null` alike.
    pub fn as_option(&self) -> Option<&T> {
        match self {
            JsonOpt::Value(value) => Some(value),
            JsonOpt::Absent | JsonOpt::Null => None,
        }
    }

    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> JsonOpt<U> {
        match self {
            JsonOpt::Absent => JsonOpt::Absent,
            JsonOpt::Null => JsonOpt::Null,
            JsonOpt::Value(value) => JsonOpt::Value(f(value)),
        }
    }

    pub fn as_ref(&self) -> JsonOpt<&T> {
        match self {
            JsonOpt::Absent => JsonOpt::Absent,
            JsonOpt::Null => JsonOpt::Null,
            JsonOpt::Value(value) => JsonOpt::Value(value),
        }
    }
}

impl<T> From<Option<T>> for JsonOpt<T> {
    fn from(value: Option<T>) -> Self {
        value.map_or(JsonOpt::Null, JsonOpt::Value)
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for JsonOpt<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Option::<T>::deserialize(deserializer).map(JsonOpt::from)
    }
}

impl<T: Serialize> Serialize for JsonOpt<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            JsonOpt::Value(value) => value.serialize(serializer),
            JsonOpt::Absent | JsonOpt::Null => serializer.serialize_none(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct Probe {
        #[serde(default, skip_serializing_if = "JsonOpt::is_absent")]
        a: JsonOpt<i64>,
    }

    #[test]
    fn keeps_absent_and_null_apart() {
        let absent: Probe = serde_json::from_str("{}").unwrap();
        let null: Probe = serde_json::from_str(r#"{"a":null}"#).unwrap();
        let value: Probe = serde_json::from_str(r#"{"a":3}"#).unwrap();
        assert_eq!(absent.a, JsonOpt::Absent);
        assert_eq!(null.a, JsonOpt::Null);
        assert_eq!(value.a, JsonOpt::Value(3));
        assert_eq!(serde_json::to_string(&absent).unwrap(), "{}");
        assert_eq!(serde_json::to_string(&null).unwrap(), r#"{"a":null}"#);
    }
}
