//! `Decoder.decodeFirstSync` parity (node-cbor 10.0.12, default options).
//!
//! The result is the JS value node hands to TS, modelled as [`JsValue`]:
//! integers stay [`JsValue::Int`] (beyond 2^53 node yields a BigInt, which
//! re-encodes the same way), floats of every width become [`JsValue::Float`],
//! tags node converts (0/1 dates, 2/3 bignums, 258 sets, 64-87 typed arrays)
//! are converted, and every other tag stays [`JsValue::Tagged`].
//!
//! Known divergences, none of which node's own encoder ever writes: tag 0
//! strings without an offset are read as UTC where node uses the process zone
//! (`Date.parse` otherwise matches V8); tag 32 (URL) and 35 (RegExp)
//! keep their original text instead of node's normalized `href`/`source`;
//! nesting deeper than [`MAX_DEPTH`] is rejected instead of recursing.

use indexmap::IndexMap;

use super::{DecodeError, JsValue, same_value_zero};

/// Maximum container/tag nesting accepted by [`decode`].
pub const MAX_DEPTH: usize = 128;

/// JS `Number.MAX_SAFE_INTEGER`.
const MAX_SAFE: u64 = 9_007_199_254_740_991;
/// node's `Tagged` rejects tags that are not 32-bit signed integers.
const MAX_TAG: u64 = 0x7fff_ffff;
/// `new Date(x)` accepts `|x| <= 8.64e15` ms.
const MAX_TIME_MS: f64 = 8.64e15;

/// Decodes exactly one CBOR item; empty input and trailing bytes are errors.
pub fn decode(bytes: &[u8]) -> Result<JsValue, DecodeError> {
    if bytes.is_empty() {
        return Err(DecodeError::Empty);
    }
    let mut reader = Reader { bytes, pos: 0 };
    let value = match reader.item(0, Parent::None)? {
        Item::Value(value) => value,
        Item::Break => return Err(invalid("Invalid BREAK")),
    };
    let rest = bytes.len() - reader.pos;
    if rest > 0 {
        return Err(DecodeError::TrailingBytes(rest));
    }
    Ok(value)
}

fn invalid(msg: impl Into<String>) -> DecodeError {
    DecodeError::Invalid(msg.into())
}

enum Item {
    Value(JsValue),
    Break,
}

/// What encloses the item being read (node's `parent[COUNT] < 0` test).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Parent {
    None,
    Definite,
    Indefinite,
}

/// The argument of an initial byte.
enum Arg {
    Value(u64),
    Indefinite,
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        let end = self.pos.checked_add(n).ok_or(DecodeError::Truncated)?;
        let slice = self
            .bytes
            .get(self.pos..end)
            .ok_or(DecodeError::Truncated)?;
        self.pos = end;
        Ok(slice)
    }

    fn byte(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }

    fn uint(&mut self, n: usize) -> Result<u64, DecodeError> {
        Ok(self
            .take(n)?
            .iter()
            .fold(0u64, |acc, b| (acc << 8) | u64::from(*b)))
    }

    fn len(&self, n: u64) -> Result<usize, DecodeError> {
        let n = usize::try_from(n).map_err(|_| DecodeError::Truncated)?;
        if n > self.bytes.len() - self.pos {
            return Err(DecodeError::Truncated);
        }
        Ok(n)
    }

    fn item(&mut self, depth: usize, parent: Parent) -> Result<Item, DecodeError> {
        if depth > MAX_DEPTH {
            return Err(invalid(format!("nesting deeper than {MAX_DEPTH}")));
        }
        let initial = self.byte()?;
        let major = initial >> 5;
        let ai = initial & 0x1f;

        if major == 7 {
            return self.simple_or_float(ai, parent).map(|v| match v {
                Some(value) => Item::Value(value),
                None => Item::Break,
            });
        }

        let arg = match ai {
            0..=23 => Arg::Value(u64::from(ai)),
            24 => Arg::Value(self.uint(1)?),
            25 => Arg::Value(self.uint(2)?),
            26 => Arg::Value(self.uint(4)?),
            27 => Arg::Value(self.uint(8)?),
            28..=30 => return Err(invalid(format!("Additional info not implemented: {ai}"))),
            _ => Arg::Indefinite,
        };

        let value = match (major, arg) {
            (0 | 1 | 6, Arg::Indefinite) => {
                return Err(invalid(format!(
                    "Invalid indefinite encoding for MT {major}"
                )));
            }
            (0, Arg::Value(n)) => JsValue::Int(i128::from(n)),
            (1, Arg::Value(n)) => JsValue::Int(-1 - i128::from(n)),
            (2, Arg::Value(n)) => {
                let n = self.len(n)?;
                JsValue::Bytes(self.take(n)?.to_vec())
            }
            (3, Arg::Value(n)) => {
                let n = self.len(n)?;
                JsValue::String(utf8(self.take(n)?)?)
            }
            (2 | 3, Arg::Indefinite) => self.indefinite_string(major)?,
            (4, Arg::Value(n)) => {
                // Every item takes at least one byte.
                let count = self.len(n)?;
                let mut items = Vec::with_capacity(count);
                for _ in 0..count {
                    items.push(self.value(depth + 1, Parent::Definite)?);
                }
                JsValue::Array(items)
            }
            (4, Arg::Indefinite) => {
                let mut items = Vec::new();
                while let Item::Value(value) = self.item(depth + 1, Parent::Indefinite)? {
                    items.push(value);
                }
                JsValue::Array(items)
            }
            (5, Arg::Value(n)) => {
                let count = self.len(n)?;
                let mut entries = Vec::with_capacity(count);
                for _ in 0..count {
                    let key = self.value(depth + 1, Parent::Definite)?;
                    let value = self.value(depth + 1, Parent::Definite)?;
                    entries.push((key, value));
                }
                build_map(entries)
            }
            (5, Arg::Indefinite) => {
                let mut entries = Vec::new();
                loop {
                    let Item::Value(key) = self.item(depth + 1, Parent::Indefinite)? else {
                        break;
                    };
                    match self.item(depth + 1, Parent::Indefinite)? {
                        Item::Value(value) => entries.push((key, value)),
                        Item::Break => {
                            return Err(invalid(format!(
                                "Invalid map length: {}",
                                entries.len() * 2 + 1
                            )));
                        }
                    }
                }
                build_map(entries)
            }
            (6, Arg::Value(tag)) => {
                // node's `Tagged` requires `(tag | 0) === tag`.
                if tag > MAX_TAG {
                    return Err(invalid(format!("Tag must be a positive integer: {tag}")));
                }
                let inner = self.value(depth + 1, Parent::Definite)?;
                convert_tag(tag, inner)
            }
            _ => return Err(invalid(format!("unexpected major type {major}"))),
        };
        Ok(Item::Value(value))
    }

    /// An item that must not be a BREAK.
    fn value(&mut self, depth: usize, parent: Parent) -> Result<JsValue, DecodeError> {
        match self.item(depth, parent)? {
            Item::Value(value) => Ok(value),
            Item::Break => Err(invalid("Invalid BREAK")),
        }
    }

    /// Major type 7. `Ok(None)` is a BREAK inside an indefinite container.
    fn simple_or_float(&mut self, ai: u8, parent: Parent) -> Result<Option<JsValue>, DecodeError> {
        let value = match ai {
            20 => JsValue::Bool(false),
            21 => JsValue::Bool(true),
            22 => JsValue::Null,
            23 => JsValue::Undefined,
            0..=19 => JsValue::Simple(ai),
            24 => {
                let simple = self.byte()?;
                if simple < 32 {
                    return Err(invalid(format!(
                        "Invalid two-byte encoding of simple value {simple}"
                    )));
                }
                JsValue::Simple(simple)
            }
            25 => {
                let half = self.take(2)?;
                JsValue::Float(parse_half(half[0], half[1]))
            }
            26 => {
                let bits = u32::try_from(self.uint(4)?).map_err(|_| DecodeError::Truncated)?;
                JsValue::Float(f64::from(f32::from_bits(bits)))
            }
            27 => JsValue::Float(f64::from_bits(self.uint(8)?)),
            28..=30 => return Err(invalid(format!("Additional info not implemented: {ai}"))),
            _ => {
                if parent == Parent::Indefinite {
                    return Ok(None);
                }
                return Err(invalid("Invalid BREAK"));
            }
        };
        Ok(Some(value))
    }

    /// Chunked byte/text string: definite chunks of the same major type.
    fn indefinite_string(&mut self, major: u8) -> Result<JsValue, DecodeError> {
        let mut bytes = Vec::new();
        loop {
            let initial = self.byte()?;
            if initial == 0xff {
                break;
            }
            if initial >> 5 != major {
                return Err(invalid("Invalid major type in indefinite encoding"));
            }
            let len = match initial & 0x1f {
                ai @ 0..=23 => u64::from(ai),
                24 => self.uint(1)?,
                25 => self.uint(2)?,
                26 => self.uint(4)?,
                27 => self.uint(8)?,
                _ => return Err(invalid("Invalid chunk in indefinite string")),
            };
            let len = self.len(len)?;
            let chunk = self.take(len)?;
            if major == 3 {
                // Each chunk is decoded (and validated) on its own by node.
                std::str::from_utf8(chunk)
                    .map_err(|e| invalid(format!("invalid UTF-8 text chunk: {e}")))?;
            }
            bytes.extend_from_slice(chunk);
        }
        if major == 3 {
            Ok(JsValue::String(utf8(&bytes)?))
        } else {
            Ok(JsValue::Bytes(bytes))
        }
    }
}

/// `TextDecoder('utf8', {fatal: true, ignoreBOM: true})`.
fn utf8(bytes: &[u8]) -> Result<String, DecodeError> {
    String::from_utf8(bytes.to_vec()).map_err(|e| invalid(format!("invalid UTF-8 text: {e}")))
}

/// node-cbor `parseHalf`.
fn parse_half(hi: u8, lo: u8) -> f64 {
    let sign = if hi & 0x80 == 0 { 1.0 } else { -1.0 };
    let exp = i32::from((hi & 0x7c) >> 2);
    let mant = f64::from((u16::from(hi & 0x03) << 8) | u16::from(lo));
    if exp == 0 {
        sign * 2f64.powi(-24) * mant
    } else if exp == 0x1f {
        if mant == 0.0 {
            sign * f64::INFINITY
        } else {
            f64::NAN
        }
    } else {
        sign * 2f64.powi(exp - 25) * (1024.0 + mant)
    }
}

/// A text-keyed map becomes a JS object unless a key is `"__proto__"`; any
/// other key type makes a JS `Map`. Duplicate keys keep the first position
/// and the last value, as property assignment and `Map#set` do.
fn build_map(entries: Vec<(JsValue, JsValue)>) -> JsValue {
    let plain = entries
        .iter()
        .all(|(key, _)| matches!(key, JsValue::String(s) if s != "__proto__"));
    if plain {
        let mut object = IndexMap::with_capacity(entries.len());
        for (key, value) in entries {
            if let JsValue::String(key) = key {
                object.insert(key, value);
            }
        }
        return JsValue::Object(object);
    }
    let mut map: Vec<(JsValue, JsValue)> = Vec::with_capacity(entries.len());
    for (key, value) in entries {
        match map
            .iter_mut()
            .find(|(existing, _)| same_value_zero(existing, &key))
        {
            Some(slot) => slot.1 = value,
            None => map.push((key, value)),
        }
    }
    JsValue::Map(map)
}

/// `TimeClip`: NaN outside ±8.64e15, otherwise truncated toward zero (no -0).
pub(crate) fn time_clip(ms: f64) -> f64 {
    if !ms.is_finite() || ms.abs() > MAX_TIME_MS {
        f64::NAN
    } else {
        ms.trunc() + 0.0
    }
}

/// JS `ToNumber` of a tag-1 payload (`v * 1000`), or `None` when node's
/// conversion throws (a BigInt operand).
fn to_number(value: &JsValue) -> Option<f64> {
    #[allow(clippy::cast_precision_loss)]
    match value {
        JsValue::Int(n) if n.unsigned_abs() <= u128::from(MAX_SAFE) => Some(*n as f64),
        JsValue::Int(_) | JsValue::BigInt(_) => None,
        JsValue::Float(x) | JsValue::Date(x) => Some(*x),
        JsValue::Null => Some(0.0),
        JsValue::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        JsValue::String(s) => Some(omni_core::js::string_to_number(s)),
        JsValue::Array(items) => Some(array_to_number(items)),
        _ => Some(f64::NAN),
    }
}

/// `Number([...])`: the array's `join(",")` as a number.
fn array_to_number(items: &[JsValue]) -> f64 {
    match items {
        [] => 0.0,
        [single] => match single {
            JsValue::Null | JsValue::Undefined => 0.0,
            JsValue::String(s) => omni_core::js::string_to_number(s),
            JsValue::Array(inner) => array_to_number(inner),
            JsValue::Int(_) | JsValue::Float(_) => to_number(single).unwrap_or(f64::NAN),
            _ => f64::NAN,
        },
        _ => f64::NAN,
    }
}

/// `new Date(s)` for a tag 0 string: `Date.parse` with zone-less date-times
/// read as local time in the process zone, as node does.
fn parse_date_string(s: &str) -> f64 {
    parse_date_string_in(s, &jiff::tz::TimeZone::system())
}

fn parse_date_string_in(s: &str, tz: &jiff::tz::TimeZone) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    omni_core::js::date_parse(s, tz).map_or(f64::NAN, |ms| ms as f64)
}

/// Big-endian magnitude bytes as an `i128`, when they fit.
fn magnitude(bytes: &[u8]) -> Option<i128> {
    let significant: &[u8] = match bytes.iter().position(|b| *b != 0) {
        Some(first) => &bytes[first..],
        None => &[],
    };
    if significant.len() > 16 || (significant.len() == 16 && significant[0] & 0x80 != 0) {
        return None;
    }
    Some(
        significant
            .iter()
            .fold(0i128, |acc, b| (acc << 8) | i128::from(*b)),
    )
}

/// `Set` construction from an iterable payload, deduplicated by SameValueZero.
fn to_set(inner: &JsValue) -> Option<Vec<JsValue>> {
    let items: Vec<JsValue> = match inner {
        JsValue::Array(items) | JsValue::Set(items) => items.clone(),
        JsValue::Null | JsValue::Undefined => Vec::new(),
        JsValue::String(s) => s.chars().map(|c| JsValue::String(c.to_string())).collect(),
        JsValue::Bytes(bytes) => bytes.iter().map(|b| JsValue::Int(i128::from(*b))).collect(),
        _ => return None,
    };
    let mut unique: Vec<JsValue> = Vec::with_capacity(items.len());
    for item in items {
        if !unique
            .iter()
            .any(|existing| same_value_zero(existing, &item))
        {
            unique.push(item);
        }
    }
    Some(unique)
}

/// RFC 8746 typed arrays node can construct: element size and whether the
/// tag is big-endian. Node re-encodes them little-endian (tag | 0b100).
fn typed_array(tag: u64) -> Option<(usize, bool)> {
    let size = match tag {
        64 | 68 | 72 => 1,
        65 | 69 | 73 | 77 => 2,
        66 | 70 | 74 | 78 | 81 | 85 => 4,
        67 | 71 | 75 | 79 | 82 | 86 => 8,
        _ => return None,
    };
    let big_endian = size > 1 && tag & 0b100 == 0;
    Some((size, big_endian))
}

/// `Tagged#convert` with node's default tag table.
fn convert_tag(tag: u64, inner: JsValue) -> JsValue {
    match tag {
        0 => match &inner {
            JsValue::String(s) => JsValue::Date(parse_date_string(s)),
            _ => match to_number(&inner) {
                Some(ms) => JsValue::Date(time_clip(ms)),
                None => JsValue::Tagged(tag, Box::new(inner)),
            },
        },
        1 => match to_number(&inner) {
            Some(seconds) => JsValue::Date(time_clip(seconds * 1000.0)),
            None => JsValue::Tagged(tag, Box::new(inner)),
        },
        2 | 3 => match &inner {
            JsValue::Bytes(bytes) if !bytes.is_empty() => match magnitude(bytes) {
                Some(n) if tag == 2 => JsValue::BigInt(n),
                Some(n) => JsValue::BigInt(-1 - n),
                None => JsValue::Tagged(tag, Box::new(inner)),
            },
            _ => JsValue::Tagged(tag, Box::new(inner)),
        },
        258 => match to_set(&inner) {
            Some(items) => JsValue::Set(items),
            None => JsValue::Tagged(tag, Box::new(inner)),
        },
        _ => match (typed_array(tag), inner) {
            (Some((size, big_endian)), JsValue::Bytes(mut bytes)) if bytes.len() % size == 0 => {
                if big_endian {
                    for element in bytes.chunks_exact_mut(size) {
                        element.reverse();
                    }
                    JsValue::Tagged(tag | 0b100, Box::new(JsValue::Bytes(bytes)))
                } else {
                    JsValue::Tagged(tag, Box::new(JsValue::Bytes(bytes)))
                }
            }
            (_, inner) => JsValue::Tagged(tag, Box::new(inner)),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::parse_date_string_in;
    use jiff::tz::TimeZone;

    #[test]
    fn tag0_strings_without_an_offset_use_the_local_zone() {
        let tz = TimeZone::get("America/Vancouver").expect("tzdb has America/Vancouver");
        // node with TZ=America/Vancouver: Date.parse("2026-09-01T03:00:00") === 1788256800000
        assert_eq!(
            parse_date_string_in("2026-09-01T03:00:00", &tz),
            1_788_256_800_000.0
        );
        // Date-only ISO strings and explicit offsets stay zone-independent.
        assert_eq!(parse_date_string_in("2026-09-01", &tz), 1_788_220_800_000.0);
        assert_eq!(
            parse_date_string_in("2026-09-01T10:00:00Z", &tz),
            1_788_256_800_000.0
        );
        assert!(parse_date_string_in("garbage", &tz).is_nan());
    }
}
