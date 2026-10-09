//! serde `Deserializer` over [`JsValue`] (see the module docs for the
//! reserved-name protocol and number coercions).

use serde::Deserialize;
use serde::de::{
    self, DeserializeOwned, DeserializeSeed, Deserializer, EnumAccess, IntoDeserializer, MapAccess,
    SeqAccess, Unexpected, VariantAccess, Visitor,
};

use super::{
    BIGINT_TOKEN, DATE_TOKEN, DecodeError, JsValue, SET_TOKEN, SIMPLE_TOKEN, TAGGED_TOKEN,
    from_value,
};

/// JS `Number.MAX_SAFE_INTEGER`.
const MAX_SAFE: i128 = 9_007_199_254_740_991;

/// `deserialize_with` for optional fields inside buffered serde contexts
/// (internally tagged/untagged enums): JS `undefined` and `null` become `None`.
pub fn undefined_as_none<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let value = JsValue::deserialize(deserializer)?;
    if value.is_nullish() {
        return Ok(None);
    }
    from_value(value).map(Some).map_err(de::Error::custom)
}

/// A serde `Deserializer` reading from an owned [`JsValue`].
pub struct ValueDeserializer {
    value: JsValue,
}

impl ValueDeserializer {
    pub fn new(value: JsValue) -> Self {
        Self { value }
    }

    fn unexpected(&self) -> Unexpected<'_> {
        match &self.value {
            JsValue::Undefined => Unexpected::Other("undefined"),
            JsValue::Null => Unexpected::Unit,
            JsValue::Bool(b) => Unexpected::Bool(*b),
            JsValue::Int(n) => match i64::try_from(*n) {
                Ok(n) => Unexpected::Signed(n),
                Err(_) => Unexpected::Other("integer beyond 64 bits"),
            },
            JsValue::Float(x) => Unexpected::Float(*x),
            JsValue::String(s) => Unexpected::Str(s),
            JsValue::Bytes(b) => Unexpected::Bytes(b),
            JsValue::Array(_) => Unexpected::Seq,
            JsValue::Object(_) | JsValue::Map(_) => Unexpected::Map,
            JsValue::Date(_) => Unexpected::Other("Date"),
            JsValue::Set(_) => Unexpected::Other("Set"),
            JsValue::BigInt(_) => Unexpected::Other("BigInt"),
            JsValue::Simple(_) => Unexpected::Other("simple value"),
            JsValue::Tagged(_, _) => Unexpected::Other("tagged value"),
        }
    }

    fn invalid<'de, V: Visitor<'de>, T>(&self, visitor: &V) -> Result<T, DecodeError> {
        Err(de::Error::invalid_type(self.unexpected(), visitor))
    }

    /// A JS-safe integer, accepting integral floats (TS numbers are doubles).
    fn safe_integer(&self) -> Option<i64> {
        match &self.value {
            JsValue::Int(n) if n.abs() <= MAX_SAFE => i64::try_from(*n).ok(),
            #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
            JsValue::Float(x) if x.fract() == 0.0 && x.abs() <= MAX_SAFE as f64 => Some(*x as i64),
            _ => None,
        }
    }

    fn integer<'de, V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DecodeError> {
        match self.safe_integer() {
            Some(n) if n >= 0 => visitor.visit_u64(n.unsigned_abs()),
            Some(n) => visitor.visit_i64(n),
            None => self.invalid(&visitor),
        }
    }

    fn token<'de, V: Visitor<'de>>(
        visitor: V,
        token: &'static str,
        payload: JsValue,
    ) -> Result<V::Value, DecodeError> {
        visitor.visit_map(TokenMap {
            key: Some(token),
            value: Some(payload),
        })
    }
}

impl<'de> Deserializer<'de> for ValueDeserializer {
    type Error = DecodeError;

    fn is_human_readable(&self) -> bool {
        true
    }

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DecodeError> {
        match self.value {
            // `none` (not a token map) so buffered contexts such as
            // `#[serde(flatten)]` read it into `Option` fields as `None`, while
            // `JsValue` keeps it apart from `null` (`visit_unit`).
            JsValue::Undefined => visitor.visit_none(),
            JsValue::Null => visitor.visit_unit(),
            JsValue::Bool(b) => visitor.visit_bool(b),
            JsValue::Int(n) => match (u64::try_from(n), i64::try_from(n)) {
                (Ok(u), _) => visitor.visit_u64(u),
                (_, Ok(i)) => visitor.visit_i64(i),
                // JS holds such an integer as a BigInt; serde's buffering
                // (flatten, tagged enums) cannot carry an i128.
                _ => Self::token(visitor, BIGINT_TOKEN, JsValue::String(n.to_string())),
            },
            JsValue::Float(x) => visitor.visit_f64(x),
            JsValue::String(s) => visitor.visit_string(s),
            JsValue::Bytes(b) => visitor.visit_byte_buf(b),
            JsValue::Array(items) => visitor.visit_seq(Seq(items.into_iter())),
            JsValue::Object(map) => visitor.visit_map(ObjectMap {
                iter: map.into_iter(),
                value: None,
            }),
            JsValue::Map(entries) => visitor.visit_map(EntryMap {
                iter: entries.into_iter(),
                value: None,
            }),
            JsValue::Date(ms) => Self::token(visitor, DATE_TOKEN, JsValue::Float(ms)),
            JsValue::Set(items) => Self::token(visitor, SET_TOKEN, JsValue::Array(items)),
            JsValue::BigInt(n) => {
                Self::token(visitor, BIGINT_TOKEN, JsValue::String(n.to_string()))
            }
            JsValue::Simple(n) => Self::token(visitor, SIMPLE_TOKEN, JsValue::Int(i128::from(n))),
            JsValue::Tagged(tag, inner) => Self::token(
                visitor,
                TAGGED_TOKEN,
                JsValue::Array(vec![JsValue::Int(i128::from(tag)), *inner]),
            ),
        }
    }

    fn deserialize_bool<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DecodeError> {
        match self.value {
            JsValue::Bool(b) => visitor.visit_bool(b),
            _ => self.invalid(&visitor),
        }
    }

    fn deserialize_i8<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DecodeError> {
        self.integer(visitor)
    }
    fn deserialize_i16<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DecodeError> {
        self.integer(visitor)
    }
    fn deserialize_i32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DecodeError> {
        self.integer(visitor)
    }
    fn deserialize_i64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DecodeError> {
        self.integer(visitor)
    }
    fn deserialize_u8<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DecodeError> {
        self.integer(visitor)
    }
    fn deserialize_u16<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DecodeError> {
        self.integer(visitor)
    }
    fn deserialize_u32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DecodeError> {
        self.integer(visitor)
    }
    fn deserialize_u64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DecodeError> {
        self.integer(visitor)
    }

    fn deserialize_i128<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DecodeError> {
        match self.value {
            JsValue::Int(n) | JsValue::BigInt(n) => visitor.visit_i128(n),
            _ => self.integer(visitor),
        }
    }
    fn deserialize_u128<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DecodeError> {
        self.deserialize_i128(visitor)
    }

    fn deserialize_f32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DecodeError> {
        self.deserialize_f64(visitor)
    }
    fn deserialize_f64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DecodeError> {
        match self.value.as_f64() {
            Some(x) => visitor.visit_f64(x),
            None => self.invalid(&visitor),
        }
    }

    fn deserialize_char<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DecodeError> {
        self.deserialize_string(visitor)
    }
    fn deserialize_str<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DecodeError> {
        self.deserialize_string(visitor)
    }
    fn deserialize_string<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DecodeError> {
        match self.value {
            JsValue::String(s) => visitor.visit_string(s),
            _ => self.invalid(&visitor),
        }
    }

    fn deserialize_bytes<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DecodeError> {
        self.deserialize_byte_buf(visitor)
    }
    fn deserialize_byte_buf<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DecodeError> {
        match self.value {
            JsValue::Bytes(bytes) => visitor.visit_byte_buf(bytes),
            JsValue::Tagged(64..=87, inner) => match *inner {
                JsValue::Bytes(bytes) => visitor.visit_byte_buf(bytes),
                other => ValueDeserializer::new(other).invalid(&visitor),
            },
            _ => self.invalid(&visitor),
        }
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DecodeError> {
        match self.value {
            JsValue::Null | JsValue::Undefined => visitor.visit_none(),
            _ => visitor.visit_some(self),
        }
    }

    fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DecodeError> {
        match self.value {
            JsValue::Null | JsValue::Undefined => visitor.visit_unit(),
            _ => self.invalid(&visitor),
        }
    }
    fn deserialize_unit_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, DecodeError> {
        self.deserialize_unit(visitor)
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        visitor: V,
    ) -> Result<V::Value, DecodeError> {
        if name == DATE_TOKEN {
            return match self.value {
                JsValue::Date(ms) => visitor.visit_f64(ms),
                _ => self.deserialize_any(visitor),
            };
        }
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DecodeError> {
        match self.value {
            JsValue::Array(items) | JsValue::Set(items) => {
                visitor.visit_seq(Seq(items.into_iter()))
            }
            _ => self.invalid(&visitor),
        }
    }
    fn deserialize_tuple<V: Visitor<'de>>(
        self,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, DecodeError> {
        self.deserialize_seq(visitor)
    }
    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, DecodeError> {
        self.deserialize_seq(visitor)
    }

    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DecodeError> {
        match self.value {
            JsValue::Object(map) => visitor.visit_map(ObjectMap {
                iter: map.into_iter(),
                value: None,
            }),
            JsValue::Map(entries) => visitor.visit_map(EntryMap {
                iter: entries.into_iter(),
                value: None,
            }),
            _ => self.invalid(&visitor),
        }
    }
    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, DecodeError> {
        self.deserialize_map(visitor)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, DecodeError> {
        match self.value {
            JsValue::String(variant) => visitor.visit_enum(Enum {
                variant,
                value: None,
            }),
            JsValue::Object(map) if map.len() == 1 => {
                let Some((variant, value)) = map.into_iter().next() else {
                    return Err(de::Error::custom("empty enum object"));
                };
                visitor.visit_enum(Enum {
                    variant,
                    value: Some(value),
                })
            }
            _ => self.invalid(&visitor),
        }
    }

    fn deserialize_identifier<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DecodeError> {
        self.deserialize_any(visitor)
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DecodeError> {
        visitor.visit_unit()
    }
}

impl IntoDeserializer<'_, DecodeError> for JsValue {
    type Deserializer = ValueDeserializer;
    fn into_deserializer(self) -> ValueDeserializer {
        ValueDeserializer::new(self)
    }
}

struct Seq(std::vec::IntoIter<JsValue>);

impl<'de> SeqAccess<'de> for Seq {
    type Error = DecodeError;
    fn next_element_seed<T: DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, DecodeError> {
        self.0
            .next()
            .map(|value| seed.deserialize(ValueDeserializer::new(value)))
            .transpose()
    }
    fn size_hint(&self) -> Option<usize> {
        Some(self.0.len())
    }
}

struct ObjectMap {
    iter: indexmap::map::IntoIter<String, JsValue>,
    value: Option<JsValue>,
}

impl<'de> MapAccess<'de> for ObjectMap {
    type Error = DecodeError;
    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, DecodeError> {
        match self.iter.next() {
            Some((key, value)) => {
                self.value = Some(value);
                seed.deserialize(ValueDeserializer::new(JsValue::String(key)))
                    .map(Some)
            }
            None => Ok(None),
        }
    }
    fn next_value_seed<V: DeserializeSeed<'de>>(
        &mut self,
        seed: V,
    ) -> Result<V::Value, DecodeError> {
        let value = self
            .value
            .take()
            .ok_or_else(|| de::Error::custom("map value requested before its key"))?;
        seed.deserialize(ValueDeserializer::new(value))
    }
    fn size_hint(&self) -> Option<usize> {
        Some(self.iter.len())
    }
}

struct EntryMap {
    iter: std::vec::IntoIter<(JsValue, JsValue)>,
    value: Option<JsValue>,
}

impl<'de> MapAccess<'de> for EntryMap {
    type Error = DecodeError;
    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, DecodeError> {
        match self.iter.next() {
            Some((key, value)) => {
                self.value = Some(value);
                seed.deserialize(ValueDeserializer::new(key)).map(Some)
            }
            None => Ok(None),
        }
    }
    fn next_value_seed<V: DeserializeSeed<'de>>(
        &mut self,
        seed: V,
    ) -> Result<V::Value, DecodeError> {
        let value = self
            .value
            .take()
            .ok_or_else(|| de::Error::custom("map value requested before its key"))?;
        seed.deserialize(ValueDeserializer::new(value))
    }
    fn size_hint(&self) -> Option<usize> {
        Some(self.iter.len())
    }
}

/// A single-entry map `{token: payload}` presenting a JS-only value.
struct TokenMap {
    key: Option<&'static str>,
    value: Option<JsValue>,
}

impl<'de> MapAccess<'de> for TokenMap {
    type Error = DecodeError;
    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, DecodeError> {
        match self.key.take() {
            Some(key) => seed
                .deserialize(ValueDeserializer::new(JsValue::String(key.to_owned())))
                .map(Some),
            None => Ok(None),
        }
    }
    fn next_value_seed<V: DeserializeSeed<'de>>(
        &mut self,
        seed: V,
    ) -> Result<V::Value, DecodeError> {
        let value = self.value.take().unwrap_or(JsValue::Null);
        seed.deserialize(ValueDeserializer::new(value))
    }
    fn size_hint(&self) -> Option<usize> {
        Some(usize::from(self.key.is_some()))
    }
}

struct Enum {
    variant: String,
    value: Option<JsValue>,
}

impl<'de> EnumAccess<'de> for Enum {
    type Error = DecodeError;
    type Variant = Variant;
    fn variant_seed<T: DeserializeSeed<'de>>(
        self,
        seed: T,
    ) -> Result<(T::Value, Variant), DecodeError> {
        let variant = seed.deserialize(ValueDeserializer::new(JsValue::String(self.variant)))?;
        Ok((variant, Variant(self.value)))
    }
}

struct Variant(Option<JsValue>);

impl<'de> VariantAccess<'de> for Variant {
    type Error = DecodeError;
    fn unit_variant(self) -> Result<(), DecodeError> {
        match self.0 {
            None | Some(JsValue::Null | JsValue::Undefined) => Ok(()),
            Some(_) => Err(de::Error::custom("expected a unit variant")),
        }
    }
    fn newtype_variant_seed<T: DeserializeSeed<'de>>(
        self,
        seed: T,
    ) -> Result<T::Value, DecodeError> {
        match self.0 {
            Some(value) => seed.deserialize(ValueDeserializer::new(value)),
            None => Err(de::Error::custom("expected a newtype variant")),
        }
    }
    fn tuple_variant<V: Visitor<'de>>(
        self,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, DecodeError> {
        match self.0 {
            Some(value) => ValueDeserializer::new(value).deserialize_seq(visitor),
            None => Err(de::Error::custom("expected a tuple variant")),
        }
    }
    fn struct_variant<V: Visitor<'de>>(
        self,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, DecodeError> {
        match self.0 {
            Some(value) => ValueDeserializer::new(value).deserialize_map(visitor),
            None => Err(de::Error::custom("expected a struct variant")),
        }
    }
}
