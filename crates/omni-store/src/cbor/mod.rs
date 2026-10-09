//! node-cbor 10.0.12 compatible codec (section 4.3).
//!
//! [`encode`] follows node's `Encoder` defaults and [`decode`] follows
//! `Decoder.decodeFirstSync`; `tests/cbor_golden.rs` checks both against
//! vectors produced by node (`scripts/golden-cbor.mjs`). [`to_value`] and
//! [`from_value`] bridge serde types and [`JsValue`].
//!
//! # Serde protocol
//! JS-only values travel through serde under reserved names:
//! - `Undefined` serializes as the unit struct [`UNDEFINED_TOKEN`] and
//!   deserializes as `none` (`null` is `unit`), so `#[serde(flatten)]` structs
//!   read an explicit `undefined` into an `Option` field as `None`;
//! - `Date` / [`JsDate`] as the newtype struct [`DATE_TOKEN`] over epoch ms (f64);
//! - `Set` as the newtype struct [`SET_TOKEN`] over a sequence;
//! - `BigInt` as the newtype struct [`BIGINT_TOKEN`] over an `i128` (the
//!   deserializer presents it as its decimal string, which serde's internal
//!   buffering can carry);
//! - `Simple` as the newtype struct [`SIMPLE_TOKEN`] over a `u8`;
//! - `Tagged` as the tuple struct [`TAGGED_TOKEN`] `(tag, value)`;
//! - `Map` (non-text keys) as a regular serde map.
//!
//! The omni deserializer's `deserialize_any` presents the other values as a
//! single-entry map whose key is the token, and the omni serializer turns such
//! a map back into the value. That keeps `#[serde(flatten)] extra: Extra`
//! lossless (serde buffers unknown fields through `deserialize_any`), but a
//! `serde_json::Value` field also sees the token maps; use `JsValue` for
//! arbitrary payloads. Typed fields are unaffected: `Option<T>` reads
//! `undefined` as `None` (buffered or not), numbers coerce between int and
//! float, and [`JsDate`] accepts tag 0/1, ISO strings and epoch ms.
//!
//! Other formats (serde_json) see plain values: `undefined` becomes `null`, a
//! Date its epoch ms, a Set an array.

mod decode;
mod value_de;
mod value_ser;

use indexmap::IndexMap;
use omni_core::js::utf16_len;
use serde::de::{self, DeserializeOwned, MapAccess, SeqAccess, Visitor};
use serde::ser::{SerializeMap, SerializeSeq, SerializeTupleStruct};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub use decode::{MAX_DEPTH, decode};
pub use value_de::ValueDeserializer;

/// Reserved serde name for JS `undefined`.
pub const UNDEFINED_TOKEN: &str = "$omni::cbor::Undefined";
/// Reserved serde name for a JS `Date` (CBOR tag 1).
pub const DATE_TOKEN: &str = "$omni::cbor::Date";
/// Reserved serde name for a JS `Set` (CBOR tag 258).
pub const SET_TOKEN: &str = "$omni::cbor::Set";
/// Reserved serde name for a JS `BigInt` (CBOR tags 2/3).
pub const BIGINT_TOKEN: &str = "$omni::cbor::BigInt";
/// Reserved serde name for a CBOR simple value other than false/true/null/undefined.
pub const SIMPLE_TOKEN: &str = "$omni::cbor::Simple";
/// Reserved serde name for any other tagged value.
pub const TAGGED_TOKEN: &str = "$omni::cbor::Tagged";

/// A decoded document as node-cbor hands it to JS.
#[derive(Clone, Debug, PartialEq)]
pub enum JsValue {
    Undefined,
    Null,
    Bool(bool),
    /// Major type 0/1 integers; `|n| <= 2^64`. Beyond `2^53 - 1` node yields a
    /// BigInt, and both re-encode identically.
    Int(i128),
    Float(f64),
    String(String),
    Bytes(Vec<u8>),
    Array(Vec<JsValue>),
    /// Text-keyed map (a JS object), insertion ordered. Encoding writes
    /// canonical array-index keys first, like `Object.keys`.
    Object(IndexMap<String, JsValue>),
    /// Map with at least one non-text key (or a `__proto__` key): a JS `Map`.
    Map(Vec<(JsValue, JsValue)>),
    /// Epoch ms as JS would hold it (`NaN` for an invalid date).
    Date(f64),
    Set(Vec<JsValue>),
    BigInt(i128),
    /// node-cbor `Simple` (values other than 20-23).
    Simple(u8),
    /// Any tag node does not convert. Typed arrays (tags 64-87) hold their
    /// little-endian bytes under the little-endian tag, as node re-encodes them.
    Tagged(u64, Box<JsValue>),
}

impl JsValue {
    /// The string, for `String` values.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            JsValue::String(s) => Some(s),
            _ => None,
        }
    }

    /// The JS number, for `Int` values within the safe range and `Float`.
    pub fn as_f64(&self) -> Option<f64> {
        #[allow(clippy::cast_precision_loss)]
        match self {
            JsValue::Int(n) if n.unsigned_abs() <= MAX_SAFE_INTEGER_U128 => Some(*n as f64),
            JsValue::Float(x) => Some(*x),
            _ => None,
        }
    }

    /// A property of an `Object`.
    pub fn get(&self, key: &str) -> Option<&JsValue> {
        match self {
            JsValue::Object(map) => map.get(key),
            _ => None,
        }
    }

    /// The object map, for `Object` values.
    pub fn as_object(&self) -> Option<&IndexMap<String, JsValue>> {
        match self {
            JsValue::Object(map) => Some(map),
            _ => None,
        }
    }

    /// The mutable object map, for `Object` values.
    pub fn as_object_mut(&mut self) -> Option<&mut IndexMap<String, JsValue>> {
        match self {
            JsValue::Object(map) => Some(map),
            _ => None,
        }
    }

    /// `null` or `undefined`.
    pub fn is_nullish(&self) -> bool {
        matches!(self, JsValue::Null | JsValue::Undefined)
    }
}

/// Unknown fields preserved by every entity (`#[serde(flatten)] extra: Extra`).
pub type Extra = IndexMap<String, JsValue>;

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("empty or NULL payload")]
    Empty,
    #[error("Insufficient data")]
    Truncated,
    #[error("{0} trailing bytes after CBOR item")]
    TrailingBytes(usize),
    #[error("invalid CBOR: {0}")]
    Invalid(String),
    #[error("{0}")]
    Message(String),
}

impl de::Error for DecodeError {
    fn custom<T: std::fmt::Display>(msg: T) -> Self {
        DecodeError::Message(msg.to_string())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EncodeError {
    #[error("{0}")]
    Message(String),
}

impl serde::ser::Error for EncodeError {
    fn custom<T: std::fmt::Display>(msg: T) -> Self {
        EncodeError::Message(msg.to_string())
    }
}

const MAX_SAFE_INTEGER_U128: u128 = 9_007_199_254_740_991;

/// `SameValueZero` (Set membership, Map keys). Containers compare by identity
/// in JS, so two of them are never equal here.
pub(crate) fn same_value_zero(a: &JsValue, b: &JsValue) -> bool {
    #[derive(PartialEq)]
    enum Prim<'a> {
        Number(f64),
        BigInt(i128),
        Str(&'a str),
        Bool(bool),
        Null,
        Undefined,
    }
    fn prim(v: &JsValue) -> Option<Prim<'_>> {
        #[allow(clippy::cast_precision_loss)]
        Some(match v {
            JsValue::Int(n) if n.unsigned_abs() <= MAX_SAFE_INTEGER_U128 => Prim::Number(*n as f64),
            JsValue::Int(n) | JsValue::BigInt(n) => Prim::BigInt(*n),
            JsValue::Float(x) => Prim::Number(*x),
            JsValue::String(s) => Prim::Str(s),
            JsValue::Bool(b) => Prim::Bool(*b),
            JsValue::Null => Prim::Null,
            JsValue::Undefined => Prim::Undefined,
            _ => return None,
        })
    }
    match (prim(a), prim(b)) {
        (Some(Prim::Number(x)), Some(Prim::Number(y))) => x == y || (x.is_nan() && y.is_nan()),
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Encoder (node `Encoder` defaults: non-canonical, dateType "number",
// collapseBigIntegers false, omitUndefinedProperties false).

const MT_POS: u8 = 0;
const MT_NEG: u8 = 1;
const MT_BYTES: u8 = 2;
const MT_TEXT: u8 = 3;
const MT_ARRAY: u8 = 4;
const MT_MAP: u8 = 5;
const MT_TAG: u8 = 6;
const MT_SIMPLE: u8 = 7;
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

/// Encodes `v` exactly as node-cbor's `encodeDoc` would encode the JS value.
pub fn encode(v: &JsValue) -> Vec<u8> {
    let mut out = Vec::new();
    push_value(&mut out, v);
    out
}

fn push_head(out: &mut Vec<u8>, major: u8, n: u64) {
    let m = major << 5;
    if n < 24 {
        out.push(m | u8::try_from(n).unwrap_or(0));
    } else if let Ok(byte) = u8::try_from(n) {
        out.push(m | 24);
        out.push(byte);
    } else if let Ok(short) = u16::try_from(n) {
        out.push(m | 25);
        out.extend_from_slice(&short.to_be_bytes());
    } else if let Ok(word) = u32::try_from(n) {
        out.push(m | 26);
        out.extend_from_slice(&word.to_be_bytes());
    } else {
        out.push(m | 27);
        out.extend_from_slice(&n.to_be_bytes());
    }
}

fn push_len(out: &mut Vec<u8>, major: u8, len: usize) {
    push_head(out, major, u64::try_from(len).unwrap_or(u64::MAX));
}

/// `_pushFloat` (non-canonical): f32 when `Math.fround(x) === x`, else f64.
fn push_float(out: &mut Vec<u8>, x: f64) {
    #[allow(clippy::cast_possible_truncation)]
    let narrowed = x as f32;
    if f64::from(narrowed) == x {
        out.push(0xfa);
        out.extend_from_slice(&narrowed.to_be_bytes());
    } else {
        out.push(0xfb);
        out.extend_from_slice(&x.to_be_bytes());
    }
}

/// `_pushNumber`: NaN/Infinity halves, -0, integers up to MAX_SAFE_INTEGER, floats.
fn push_number(out: &mut Vec<u8>, x: f64) {
    if x.is_nan() {
        out.extend_from_slice(&[0xf9, 0x7e, 0x00]);
    } else if x.is_infinite() {
        out.extend_from_slice(if x < 0.0 {
            &[0xf9, 0xfc, 0x00]
        } else {
            &[0xf9, 0x7c, 0x00]
        });
    } else if x.round() == x {
        if x == 0.0 && x.is_sign_negative() {
            out.extend_from_slice(&[0xf9, 0x80, 0x00]);
        } else if x >= 0.0 {
            if x <= MAX_SAFE_INTEGER {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                push_head(out, MT_POS, x as u64);
            } else {
                push_float(out, x);
            }
        } else {
            let magnitude = -x - 1.0;
            if magnitude <= MAX_SAFE_INTEGER - 1.0 {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                push_head(out, MT_NEG, magnitude as u64);
            } else {
                push_float(out, x);
            }
        }
    } else {
        push_float(out, x);
    }
}

/// `_pushJSBigint` with `collapseBigIntegers: false`: always tag 2/3 + bytes.
fn push_bigint(out: &mut Vec<u8>, n: i128) {
    let (tag, magnitude) = if n < 0 {
        (3, (-(n + 1)).unsigned_abs())
    } else {
        (2, n.unsigned_abs())
    };
    let bytes = magnitude.to_be_bytes();
    let first = bytes
        .iter()
        .position(|b| *b != 0)
        .unwrap_or(bytes.len() - 1);
    push_head(out, MT_TAG, tag);
    push_len(out, MT_BYTES, bytes.len() - first);
    out.extend_from_slice(&bytes[first..]);
}

fn push_int(out: &mut Vec<u8>, n: i128) {
    // Integers within the safe range came from (and go back to) JS numbers.
    // Larger 64-bit values are BigInts in node-cbor's decoder, so they are
    // re-encoded the way node encodes a BigInt.
    const SAFE: i128 = 9_007_199_254_740_991;
    if (0..=SAFE).contains(&n) {
        push_head(out, MT_POS, u64::try_from(n).unwrap_or(0));
    } else if (-SAFE..0).contains(&n) {
        push_head(out, MT_NEG, u64::try_from(-(n + 1)).unwrap_or(0));
    } else {
        push_bigint(out, n);
    }
}

fn push_text(out: &mut Vec<u8>, s: &str) {
    push_len(out, MT_TEXT, s.len());
    out.extend_from_slice(s.as_bytes());
}

/// Canonical array index (`"0"` or no leading zero, below 2^32 - 1).
fn array_index(key: &str) -> Option<u32> {
    if key.is_empty() || key.len() > 10 || (key.len() > 1 && key.starts_with('0')) {
        return None;
    }
    if !key.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    key.parse::<u64>()
        .ok()
        .filter(|n| *n < u64::from(u32::MAX))
        .and_then(|n| u32::try_from(n).ok())
}

/// Canonical array index keys first (ascending), then insertion order: the
/// order `Object.keys` yields and node-cbor writes.
pub(crate) fn js_key_order(map: &IndexMap<String, JsValue>) -> Vec<(&String, &JsValue)> {
    let mut indexed: Vec<(u32, &String, &JsValue)> = Vec::new();
    let mut named = Vec::new();
    for (key, value) in map {
        match array_index(key) {
            Some(i) => indexed.push((i, key, value)),
            None => named.push((key, value)),
        }
    }
    if indexed.is_empty() {
        return named;
    }
    indexed.sort_by_key(|(i, _, _)| *i);
    indexed
        .into_iter()
        .map(|(_, k, v)| (k, v))
        .chain(named)
        .collect()
}

fn push_value(out: &mut Vec<u8>, v: &JsValue) {
    match v {
        JsValue::Undefined => out.push(0xf7),
        JsValue::Null => out.push(0xf6),
        JsValue::Bool(b) => out.push(if *b { 0xf5 } else { 0xf4 }),
        JsValue::Int(n) => push_int(out, *n),
        JsValue::Float(x) => push_number(out, *x),
        JsValue::String(s) => push_text(out, s),
        JsValue::Bytes(bytes) => {
            push_len(out, MT_BYTES, bytes.len());
            out.extend_from_slice(bytes);
        }
        JsValue::Array(items) => {
            push_len(out, MT_ARRAY, items.len());
            for item in items {
                push_value(out, item);
            }
        }
        JsValue::Object(map) => {
            push_len(out, MT_MAP, map.len());
            for (key, value) in js_key_order(map) {
                push_text(out, key);
                push_value(out, value);
            }
        }
        JsValue::Map(entries) => {
            push_len(out, MT_MAP, entries.len());
            for (key, value) in entries {
                push_value(out, key);
                push_value(out, value);
            }
        }
        JsValue::Date(ms) => {
            push_head(out, MT_TAG, 1);
            push_number(out, ms / 1000.0);
        }
        JsValue::Set(items) => {
            push_head(out, MT_TAG, 258);
            push_len(out, MT_ARRAY, items.len());
            for item in items {
                push_value(out, item);
            }
        }
        JsValue::BigInt(n) => push_bigint(out, *n),
        JsValue::Simple(n) => push_head(out, MT_SIMPLE, u64::from(*n)),
        JsValue::Tagged(tag, inner) => {
            push_head(out, MT_TAG, *tag);
            push_value(out, inner);
        }
    }
}

/// Serializes `t` into the JS value TS would have built: struct fields in
/// declaration order, `None` as `null` unless skipped, unit variants as
/// strings, maps with only string keys as objects.
pub fn to_value<T: Serialize + ?Sized>(t: &T) -> Result<JsValue, EncodeError> {
    t.serialize(value_ser::ValueSerializer)
}

/// Self-describing deserialization of a decoded document into `T`.
pub fn from_value<T: DeserializeOwned>(v: JsValue) -> Result<T, DecodeError> {
    T::deserialize(value_de::ValueDeserializer::new(v))
}

/// The JS string length of a value's text, used by key encoding.
pub(crate) fn js_len(s: &str) -> usize {
    utf16_len(s)
}

// ---------------------------------------------------------------------------
// JsDate

/// A JS `Date` field: epoch ms, written as CBOR tag 1. Deserializes from tag
/// 0/1, an ISO string or a number of epoch ms. An invalid date (NaN) is a
/// deserialization error; use `JsValue` where invalid dates must survive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct JsDate(pub i64);

impl Serialize for JsDate {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[allow(clippy::cast_precision_loss)]
        let ms = self.0 as f64;
        serializer.serialize_newtype_struct(DATE_TOKEN, &ms)
    }
}

struct JsDateVisitor;

impl<'de> Visitor<'de> for JsDateVisitor {
    type Value = JsDate;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a date as epoch ms or an ISO-8601 string")
    }

    fn visit_newtype_struct<D: Deserializer<'de>>(self, d: D) -> Result<JsDate, D::Error> {
        d.deserialize_any(self)
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<JsDate, E> {
        Ok(JsDate(v))
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<JsDate, E> {
        i64::try_from(v)
            .map(JsDate)
            .map_err(|_| E::custom("date out of range"))
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<JsDate, E> {
        if !v.is_finite() {
            return Err(E::custom("invalid date"));
        }
        // `new Date(x)` truncates toward zero.
        #[allow(clippy::cast_possible_truncation)]
        let ms = v.trunc() as i64;
        Ok(JsDate(ms))
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<JsDate, E> {
        v.parse::<jiff::Timestamp>()
            .map(|t| JsDate(t.as_millisecond()))
            .map_err(|e| E::custom(format!("invalid date string: {e}")))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<JsDate, A::Error> {
        match map.next_key::<String>()? {
            Some(key) if key == DATE_TOKEN => {
                let ms: f64 = map.next_value()?;
                self.visit_f64(ms)
            }
            _ => Err(de::Error::custom("expected a date")),
        }
    }
}

impl<'de> Deserialize<'de> for JsDate {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_newtype_struct(DATE_TOKEN, JsDateVisitor)
    }
}

// ---------------------------------------------------------------------------
// serde for JsValue (see the module docs for the reserved-name protocol)

struct TaggedRef<'a>(u64, &'a JsValue);

impl Serialize for TaggedRef<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_tuple_struct(TAGGED_TOKEN, 2)?;
        state.serialize_field(&self.0)?;
        state.serialize_field(self.1)?;
        state.end()
    }
}

struct SetRef<'a>(&'a [JsValue]);

impl Serialize for SetRef<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.0.len()))?;
        for item in self.0 {
            seq.serialize_element(item)?;
        }
        seq.end()
    }
}

impl Serialize for JsValue {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            JsValue::Undefined => serializer.serialize_unit_struct(UNDEFINED_TOKEN),
            JsValue::Null => serializer.serialize_unit(),
            JsValue::Bool(b) => serializer.serialize_bool(*b),
            JsValue::Int(n) => match i64::try_from(*n) {
                Ok(small) => serializer.serialize_i64(small),
                Err(_) => serializer.serialize_i128(*n),
            },
            JsValue::Float(x) => serializer.serialize_f64(*x),
            JsValue::String(s) => serializer.serialize_str(s),
            JsValue::Bytes(bytes) => serializer.serialize_bytes(bytes),
            JsValue::Array(items) => {
                let mut seq = serializer.serialize_seq(Some(items.len()))?;
                for item in items {
                    seq.serialize_element(item)?;
                }
                seq.end()
            }
            JsValue::Object(map) => {
                let mut state = serializer.serialize_map(Some(map.len()))?;
                for (key, value) in map {
                    state.serialize_entry(key, value)?;
                }
                state.end()
            }
            JsValue::Map(entries) => {
                let mut state = serializer.serialize_map(Some(entries.len()))?;
                for (key, value) in entries {
                    state.serialize_entry(key, value)?;
                }
                state.end()
            }
            JsValue::Date(ms) => serializer.serialize_newtype_struct(DATE_TOKEN, ms),
            JsValue::Set(items) => serializer.serialize_newtype_struct(SET_TOKEN, &SetRef(items)),
            JsValue::BigInt(n) => serializer.serialize_newtype_struct(BIGINT_TOKEN, n),
            JsValue::Simple(n) => serializer.serialize_newtype_struct(SIMPLE_TOKEN, n),
            JsValue::Tagged(tag, inner) => TaggedRef(*tag, inner).serialize(serializer),
        }
    }
}

struct JsValueVisitor;

impl<'de> Visitor<'de> for JsValueVisitor {
    type Value = JsValue;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("any CBOR/JS value")
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<JsValue, E> {
        Ok(JsValue::Bool(v))
    }
    fn visit_i64<E: de::Error>(self, v: i64) -> Result<JsValue, E> {
        Ok(JsValue::Int(i128::from(v)))
    }
    fn visit_i128<E: de::Error>(self, v: i128) -> Result<JsValue, E> {
        Ok(JsValue::Int(v))
    }
    fn visit_u64<E: de::Error>(self, v: u64) -> Result<JsValue, E> {
        Ok(JsValue::Int(i128::from(v)))
    }
    fn visit_u128<E: de::Error>(self, v: u128) -> Result<JsValue, E> {
        i128::try_from(v)
            .map(JsValue::Int)
            .map_err(|_| E::custom("integer out of range"))
    }
    fn visit_f64<E: de::Error>(self, v: f64) -> Result<JsValue, E> {
        Ok(JsValue::Float(v))
    }
    fn visit_str<E: de::Error>(self, v: &str) -> Result<JsValue, E> {
        Ok(JsValue::String(v.to_owned()))
    }
    fn visit_string<E: de::Error>(self, v: String) -> Result<JsValue, E> {
        Ok(JsValue::String(v))
    }
    fn visit_bytes<E: de::Error>(self, v: &[u8]) -> Result<JsValue, E> {
        Ok(JsValue::Bytes(v.to_vec()))
    }
    fn visit_byte_buf<E: de::Error>(self, v: Vec<u8>) -> Result<JsValue, E> {
        Ok(JsValue::Bytes(v))
    }
    /// The omni deserializer presents `undefined` as `none` and `null` as `unit`.
    fn visit_none<E: de::Error>(self) -> Result<JsValue, E> {
        Ok(JsValue::Undefined)
    }
    fn visit_unit<E: de::Error>(self) -> Result<JsValue, E> {
        Ok(JsValue::Null)
    }
    fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<JsValue, D::Error> {
        JsValue::deserialize(d)
    }
    fn visit_newtype_struct<D: Deserializer<'de>>(self, d: D) -> Result<JsValue, D::Error> {
        JsValue::deserialize(d)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<JsValue, A::Error> {
        let mut items = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(4096));
        while let Some(item) = seq.next_element::<JsValue>()? {
            items.push(item);
        }
        Ok(JsValue::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<JsValue, A::Error> {
        let Some(first_key) = map.next_key::<JsValue>()? else {
            return Ok(JsValue::Object(IndexMap::new()));
        };
        if let JsValue::String(token) = &first_key {
            match token.as_str() {
                UNDEFINED_TOKEN => {
                    map.next_value::<de::IgnoredAny>()?;
                    return Ok(JsValue::Undefined);
                }
                DATE_TOKEN => return Ok(JsValue::Date(map.next_value::<f64>()?)),
                SET_TOKEN => return Ok(JsValue::Set(map.next_value::<Vec<JsValue>>()?)),
                BIGINT_TOKEN => {
                    let digits = map.next_value::<String>()?;
                    return digits
                        .parse::<i128>()
                        .map(JsValue::BigInt)
                        .map_err(|_| de::Error::custom("invalid BigInt digits"));
                }
                SIMPLE_TOKEN => return Ok(JsValue::Simple(map.next_value::<u8>()?)),
                TAGGED_TOKEN => {
                    let (tag, inner) = map.next_value::<(u64, JsValue)>()?;
                    return Ok(JsValue::Tagged(tag, Box::new(inner)));
                }
                _ => {}
            }
        }
        let mut entries = vec![(first_key, map.next_value::<JsValue>()?)];
        while let Some((key, value)) = map.next_entry::<JsValue, JsValue>()? {
            entries.push((key, value));
        }
        let plain = entries
            .iter()
            .all(|(key, _)| matches!(key, JsValue::String(s) if s != "__proto__"));
        if plain {
            let object = entries
                .into_iter()
                .filter_map(|(key, value)| match key {
                    JsValue::String(key) => Some((key, value)),
                    _ => None,
                })
                .collect();
            Ok(JsValue::Object(object))
        } else {
            Ok(JsValue::Map(entries))
        }
    }
}

impl<'de> Deserialize<'de> for JsValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(JsValueVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    // Expected bytes produced with node: require("cbor").encode(...) (cbor 10.0.12).
    #[test]
    fn numbers_follow_node_rules() {
        assert_eq!(hex(&encode(&JsValue::Float(0.0))), "00");
        assert_eq!(hex(&encode(&JsValue::Float(-0.0))), "f98000");
        assert_eq!(hex(&encode(&JsValue::Float(23.0))), "17");
        assert_eq!(hex(&encode(&JsValue::Float(24.0))), "1818");
        assert_eq!(hex(&encode(&JsValue::Float(-1.0))), "20");
        assert_eq!(hex(&encode(&JsValue::Float(1.5))), "fa3fc00000");
        assert_eq!(hex(&encode(&JsValue::Float(0.1))), "fb3fb999999999999a");
        assert_eq!(hex(&encode(&JsValue::Float(f64::NAN))), "f97e00");
        assert_eq!(hex(&encode(&JsValue::Float(f64::NEG_INFINITY))), "f9fc00");
        assert_eq!(
            hex(&encode(&JsValue::Float(9_007_199_254_740_991.0))),
            "1b001fffffffffffff"
        );
        assert_eq!(
            hex(&encode(&JsValue::Float(9_007_199_254_740_992.0))),
            "fa5a000000"
        );
        assert_eq!(
            hex(&encode(&JsValue::Float(1_760_000_000_123.0))),
            "1b00000199c82cc07b"
        );
        assert_eq!(
            hex(&encode(&JsValue::Float(-9_007_199_254_740_991.0))),
            "3b001ffffffffffffe"
        );
        assert_eq!(
            hex(&encode(&JsValue::Float(-9_007_199_254_740_992.0))),
            "fada000000"
        );
    }

    #[test]
    fn containers_dates_and_sets() {
        let mut object = IndexMap::new();
        object.insert("b".to_owned(), JsValue::Undefined);
        object.insert("1".to_owned(), JsValue::Null);
        object.insert("a".to_owned(), JsValue::String("é".to_owned()));
        assert_eq!(
            hex(&encode(&JsValue::Object(object))),
            "a36131f66162f7616162c3a9"
        );
        assert_eq!(hex(&encode(&JsValue::Date(1_000.0))), "c101");
        assert_eq!(hex(&encode(&JsValue::Date(1_500.0))), "c1fa3fc00000");
        assert_eq!(
            hex(&encode(&JsValue::Set(vec![JsValue::Bool(true)]))),
            "d9010281f5"
        );
        assert_eq!(hex(&encode(&JsValue::BigInt(1))), "c24101");
        assert_eq!(hex(&encode(&JsValue::BigInt(-1))), "c34100");
        assert_eq!(hex(&encode(&JsValue::Simple(16))), "f0");
        assert_eq!(hex(&encode(&JsValue::Simple(255))), "f8ff");
    }

    #[test]
    fn same_value_zero_follows_js() {
        assert!(same_value_zero(&JsValue::Int(1), &JsValue::Float(1.0)));
        assert!(same_value_zero(&JsValue::Float(-0.0), &JsValue::Int(0)));
        assert!(same_value_zero(
            &JsValue::Float(f64::NAN),
            &JsValue::Float(f64::NAN)
        ));
        assert!(!same_value_zero(&JsValue::Int(1), &JsValue::BigInt(1)));
        assert!(!same_value_zero(
            &JsValue::Array(vec![]),
            &JsValue::Array(vec![])
        ));
    }
}
