//! Constrained string types shared by records, events and tool inputs. Each
//! serializes as a plain string and advertises its pattern in the tool schemas;
//! [`check`] re-validates values that did not come through an MCP schema.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};

macro_rules! constrained_string {
    ($(#[$meta:meta])* $name:ident, $pattern:expr, $min:expr, $max:expr) => {
        $(#[$meta])*
        #[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            pub const PATTERN: &'static str = $pattern;

            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Validates `value` against the advertised pattern and length.
            pub fn parse(value: &str) -> Result<Self, String> {
                check(value, Self::PATTERN, $min, $max)
                    .map(|()| Self(value.to_owned()))
                    .map_err(|reason| format!("{} {reason}", stringify!($name)))
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl JsonSchema for $name {
            fn inline_schema() -> bool {
                true
            }

            fn schema_name() -> Cow<'static, str> {
                stringify!($name).into()
            }

            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                json_schema!({
                    "type": "string",
                    "minLength": $min,
                    "maxLength": $max,
                    "pattern": $pattern,
                })
            }
        }
    };
}

constrained_string!(
    /// `real` (the owner's ledger) or `fixture-<slug>` (disposable test data).
    Namespace,
    "^(real|fixture-[a-z0-9][a-z0-9-]{0,39})$",
    4,
    48
);

constrained_string!(
    /// A caller-chosen key for one logical write: 16-128 of `[A-Za-z0-9_-]`.
    IdempotencyKey,
    "^[A-Za-z0-9_-]{16,128}$",
    16,
    128
);

constrained_string!(
    /// A record id (`var_`, `pc_`, `ms_`, `pf_`, `pref_`, `pred_`, `rule_`, `need_`).
    RecordId,
    "^(var|pc|ms|pf|pref|pred|rule|need)_[0-9a-f]{32}$",
    35,
    37
);

constrained_string!(
    /// An event id (`ev_`).
    EventId,
    "^ev_[0-9a-f]{32}$",
    35,
    35
);

constrained_string!(
    /// An evidence id (`evd_`).
    EvidenceId,
    "^evd_[0-9a-f]{32}$",
    36,
    36
);

constrained_string!(
    /// Any ledger id: a record, event or evidence id.
    LedgerId,
    "^(var|pc|ms|pf|pref|pred|rule|need|ev|evd)_[0-9a-f]{32}$",
    35,
    37
);

constrained_string!(
    /// A lowercase identifier such as a region or dimension (`chest`, `body_length`).
    Slug,
    "^[a-z0-9][a-z0-9_]{0,47}$",
    1,
    48
);

constrained_string!(
    /// A category path such as `tops/tshirt` or `bottoms/jeans`.
    Category,
    "^[a-z0-9][a-z0-9_]{0,31}(/[a-z0-9][a-z0-9_]{0,31}){0,3}$",
    1,
    131
);

constrained_string!(
    /// An ISO 4217 currency code.
    Currency,
    "^[A-Z]{3}$",
    3,
    3
);

constrained_string!(
    /// A lowercase hex SHA-256 digest.
    Sha256Hex,
    "^[0-9a-f]{64}$",
    64,
    64
);

constrained_string!(
    /// A record kind, event type or evidence kind name.
    KindName,
    "^[a-z][a-z_]{0,31}$",
    1,
    32
);

constrained_string!(
    /// A stored archive id from `stylist_export` or `stylist_archives_list`.
    ArchiveId,
    "^[a-z0-9][a-z0-9-]{0,119}$",
    1,
    120
);

/// Validates `value` against one of the patterns above. They use only ASCII
/// classes, so Rust regex semantics match the JSON-schema engine that checks
/// the same patterns at the MCP boundary.
pub fn check(value: &str, pattern: &'static str, min: usize, max: usize) -> Result<(), String> {
    let length = value.chars().count();
    if length < min || length > max {
        return Err(format!("must be {min}-{max} characters"));
    }
    if !compiled(pattern).is_some_and(|re| re.is_match(value)) {
        return Err(format!("must match {pattern}"));
    }
    Ok(())
}

/// The compiled pattern, built once per pattern.
fn compiled(pattern: &'static str) -> Option<&'static regex::Regex> {
    type Cache = Mutex<HashMap<&'static str, &'static regex::Regex>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let mut cache = CACHE.get_or_init(Cache::default).lock().ok()?;
    if let Some(re) = cache.get(pattern) {
        return Some(re);
    }
    let re: &'static regex::Regex = Box::leak(Box::new(regex::Regex::new(pattern).ok()?));
    cache.insert(pattern, re);
    Some(re)
}

/// A new random id with `prefix`.
pub fn new_id(prefix: &str) -> String {
    format!("{prefix}_{}", uuid::Uuid::new_v4().simple())
}

impl Namespace {
    pub fn is_fixture(&self) -> bool {
        self.0.starts_with("fixture-")
    }

    pub fn real() -> Self {
        Self("real".to_owned())
    }
}

impl LedgerId {
    /// The id's prefix (`var`, `ev`, ...).
    pub fn prefix(&self) -> &str {
        self.0.split_once('_').map_or("", |(p, _)| p)
    }
}

impl RecordId {
    pub fn as_ledger(&self) -> LedgerId {
        LedgerId(self.0.clone())
    }
}

impl EventId {
    pub fn as_ledger(&self) -> LedgerId {
        LedgerId(self.0.clone())
    }
}

impl EvidenceId {
    pub fn as_ledger(&self) -> LedgerId {
        LedgerId(self.0.clone())
    }
}

impl Category {
    /// `self` equals `prefix` or lies below it (`tops/tshirt` is under `tops`).
    pub fn is_under(&self, prefix: &str) -> bool {
        self.0 == prefix
            || self
                .0
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.starts_with('/'))
    }

    pub fn top(&self) -> &str {
        self.0.split('/').next().unwrap_or(&self.0)
    }
}

/// Parses a `YYYY-MM-DD` date.
pub fn parse_date(value: &str) -> Result<jiff::civil::Date, String> {
    if value.len() != 10 {
        return Err(format!("invalid date {value:?} (YYYY-MM-DD)"));
    }
    value
        .parse::<jiff::civil::Date>()
        .map_err(|_| format!("invalid date {value:?} (YYYY-MM-DD)"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_accept_and_reject() {
        assert!(Namespace::parse("real").is_ok());
        assert!(Namespace::parse("fixture-acceptance-1").is_ok());
        assert!(Namespace::parse("fixture-").is_err());
        assert!(Namespace::parse("Real").is_err());
        assert!(Namespace::parse("prod").is_err());
        let id = new_id("pc");
        assert!(RecordId::parse(&id).is_ok());
        assert!(LedgerId::parse(&id).is_ok());
        assert!(EventId::parse(&id).is_err());
        assert!(EventId::parse(&new_id("ev")).is_ok());
        assert!(EvidenceId::parse(&new_id("evd")).is_ok());
        assert!(Slug::parse("body_length").is_ok());
        assert!(Slug::parse("Body").is_err());
        assert!(Category::parse("tops/tshirt").is_ok());
        assert!(Category::parse("tops//tshirt").is_err());
        assert!(Category::parse("a/b/c/d/e").is_err());
        assert!(IdempotencyKey::parse("short").is_err());
        assert!(IdempotencyKey::parse("acceptance-key-0001").is_ok());
        assert!(Currency::parse("USD").is_ok());
        assert!(Currency::parse("usd").is_err());
    }

    #[test]
    fn categories_nest() {
        let category = Category("tops/tshirt".to_owned());
        assert!(category.is_under("tops"));
        assert!(category.is_under("tops/tshirt"));
        assert!(!category.is_under("top"));
        assert_eq!(category.top(), "tops");
    }

    #[test]
    fn dates_parse_strictly() {
        assert!(parse_date("2026-10-12").is_ok());
        assert!(parse_date("2026-02-30").is_err());
        assert!(parse_date("2026-1-2").is_err());
    }
}
