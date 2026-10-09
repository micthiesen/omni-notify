//! serde `Serializer` producing [`JsValue`] (the JS value TS would build).

use indexmap::IndexMap;
use serde::Serialize;
use serde::ser;

use super::{
    BIGINT_TOKEN, DATE_TOKEN, EncodeError, JsValue, SET_TOKEN, SIMPLE_TOKEN, TAGGED_TOKEN,
    UNDEFINED_TOKEN,
};

pub(crate) struct ValueSerializer;

fn err(msg: impl Into<String>) -> EncodeError {
    EncodeError::Message(msg.into())
}

/// Turns a single-entry token map (the deserializer's presentation of a JS
/// value) back into that value.
fn from_token_map(key: &str, value: JsValue) -> Result<JsValue, (String, JsValue)> {
    match (key, value) {
        (UNDEFINED_TOKEN, _) => Ok(JsValue::Undefined),
        (DATE_TOKEN, value) => match value.as_f64() {
            Some(ms) => Ok(JsValue::Date(ms)),
            None => Err((key.to_owned(), value)),
        },
        (SET_TOKEN, JsValue::Array(items)) => Ok(JsValue::Set(items)),
        (BIGINT_TOKEN, JsValue::Int(n)) => Ok(JsValue::BigInt(n)),
        (BIGINT_TOKEN, JsValue::String(digits)) => match digits.parse::<i128>() {
            Ok(n) => Ok(JsValue::BigInt(n)),
            Err(_) => Err((key.to_owned(), JsValue::String(digits))),
        },
        (SIMPLE_TOKEN, JsValue::Int(n)) => match u8::try_from(n) {
            Ok(simple) => Ok(JsValue::Simple(simple)),
            Err(_) => Err((key.to_owned(), JsValue::Int(n))),
        },
        (TAGGED_TOKEN, JsValue::Array(mut pair)) if pair.len() == 2 => {
            let inner = pair.pop().unwrap_or(JsValue::Undefined);
            match pair.pop() {
                Some(JsValue::Int(tag)) if tag >= 0 => match u64::try_from(tag) {
                    Ok(tag) => Ok(JsValue::Tagged(tag, Box::new(inner))),
                    Err(_) => Err((
                        key.to_owned(),
                        JsValue::Array(vec![JsValue::Int(tag), inner]),
                    )),
                },
                Some(other) => Err((key.to_owned(), JsValue::Array(vec![other, inner]))),
                None => Err((key.to_owned(), JsValue::Array(vec![inner]))),
            }
        }
        (_, value) => Err((key.to_owned(), value)),
    }
}

fn finish_map(mut entries: Vec<(JsValue, JsValue)>) -> JsValue {
    let token_map = entries.len() == 1
        && matches!(entries.first(), Some((JsValue::String(key), _)) if key.starts_with("$omni::cbor::"));
    if token_map && let Some((JsValue::String(key), value)) = entries.pop() {
        return match from_token_map(&key, value) {
            Ok(value) => value,
            Err((key, value)) => {
                let mut object = IndexMap::new();
                object.insert(key, value);
                JsValue::Object(object)
            }
        };
    }
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
        JsValue::Object(object)
    } else {
        JsValue::Map(entries)
    }
}

impl ser::Serializer for ValueSerializer {
    type Ok = JsValue;
    type Error = EncodeError;
    type SerializeSeq = SeqSer;
    type SerializeTuple = SeqSer;
    type SerializeTupleStruct = TupleStructSer;
    type SerializeTupleVariant = VariantSer<SeqSer>;
    type SerializeMap = MapSer;
    type SerializeStruct = StructSer;
    type SerializeStructVariant = VariantSer<StructSer>;

    fn is_human_readable(&self) -> bool {
        true
    }

    fn serialize_bool(self, v: bool) -> Result<JsValue, EncodeError> {
        Ok(JsValue::Bool(v))
    }
    fn serialize_i8(self, v: i8) -> Result<JsValue, EncodeError> {
        Ok(JsValue::Int(i128::from(v)))
    }
    fn serialize_i16(self, v: i16) -> Result<JsValue, EncodeError> {
        Ok(JsValue::Int(i128::from(v)))
    }
    fn serialize_i32(self, v: i32) -> Result<JsValue, EncodeError> {
        Ok(JsValue::Int(i128::from(v)))
    }
    fn serialize_i64(self, v: i64) -> Result<JsValue, EncodeError> {
        Ok(JsValue::Int(i128::from(v)))
    }
    fn serialize_i128(self, v: i128) -> Result<JsValue, EncodeError> {
        Ok(JsValue::Int(v))
    }
    fn serialize_u8(self, v: u8) -> Result<JsValue, EncodeError> {
        Ok(JsValue::Int(i128::from(v)))
    }
    fn serialize_u16(self, v: u16) -> Result<JsValue, EncodeError> {
        Ok(JsValue::Int(i128::from(v)))
    }
    fn serialize_u32(self, v: u32) -> Result<JsValue, EncodeError> {
        Ok(JsValue::Int(i128::from(v)))
    }
    fn serialize_u64(self, v: u64) -> Result<JsValue, EncodeError> {
        Ok(JsValue::Int(i128::from(v)))
    }
    fn serialize_u128(self, v: u128) -> Result<JsValue, EncodeError> {
        i128::try_from(v)
            .map(JsValue::Int)
            .map_err(|_| err("u128 value exceeds the CBOR integer range"))
    }
    fn serialize_f32(self, v: f32) -> Result<JsValue, EncodeError> {
        Ok(JsValue::Float(f64::from(v)))
    }
    fn serialize_f64(self, v: f64) -> Result<JsValue, EncodeError> {
        Ok(JsValue::Float(v))
    }
    fn serialize_char(self, v: char) -> Result<JsValue, EncodeError> {
        Ok(JsValue::String(v.to_string()))
    }
    fn serialize_str(self, v: &str) -> Result<JsValue, EncodeError> {
        Ok(JsValue::String(v.to_owned()))
    }
    fn serialize_bytes(self, v: &[u8]) -> Result<JsValue, EncodeError> {
        Ok(JsValue::Bytes(v.to_vec()))
    }
    fn serialize_none(self) -> Result<JsValue, EncodeError> {
        Ok(JsValue::Null)
    }
    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<JsValue, EncodeError> {
        value.serialize(self)
    }
    fn serialize_unit(self) -> Result<JsValue, EncodeError> {
        Ok(JsValue::Null)
    }
    fn serialize_unit_struct(self, name: &'static str) -> Result<JsValue, EncodeError> {
        Ok(if name == UNDEFINED_TOKEN {
            JsValue::Undefined
        } else {
            JsValue::Null
        })
    }
    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
    ) -> Result<JsValue, EncodeError> {
        Ok(JsValue::String(variant.to_owned()))
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        name: &'static str,
        value: &T,
    ) -> Result<JsValue, EncodeError> {
        let inner = value.serialize(ValueSerializer)?;
        match name {
            DATE_TOKEN => inner
                .as_f64()
                .map(JsValue::Date)
                .ok_or_else(|| err("date must serialize as epoch ms")),
            SET_TOKEN => match inner {
                JsValue::Array(items) => Ok(JsValue::Set(items)),
                _ => Err(err("set must serialize as a sequence")),
            },
            BIGINT_TOKEN => match inner {
                JsValue::Int(n) => Ok(JsValue::BigInt(n)),
                _ => Err(err("bigint must serialize as an integer")),
            },
            SIMPLE_TOKEN => match inner {
                JsValue::Int(n) => u8::try_from(n)
                    .map(JsValue::Simple)
                    .map_err(|_| err("simple value out of range")),
                _ => Err(err("simple value must serialize as an integer")),
            },
            _ => Ok(inner),
        }
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<JsValue, EncodeError> {
        let mut object = IndexMap::new();
        object.insert(variant.to_owned(), value.serialize(ValueSerializer)?);
        Ok(JsValue::Object(object))
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<SeqSer, EncodeError> {
        Ok(SeqSer(Vec::with_capacity(len.unwrap_or(0).min(4096))))
    }
    fn serialize_tuple(self, len: usize) -> Result<SeqSer, EncodeError> {
        self.serialize_seq(Some(len))
    }
    fn serialize_tuple_struct(
        self,
        name: &'static str,
        len: usize,
    ) -> Result<TupleStructSer, EncodeError> {
        Ok(TupleStructSer {
            tagged: name == TAGGED_TOKEN,
            items: Vec::with_capacity(len.min(4096)),
        })
    }
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<VariantSer<SeqSer>, EncodeError> {
        Ok(VariantSer {
            variant,
            inner: SeqSer(Vec::with_capacity(len.min(4096))),
        })
    }
    fn serialize_map(self, len: Option<usize>) -> Result<MapSer, EncodeError> {
        Ok(MapSer {
            entries: Vec::with_capacity(len.unwrap_or(0).min(4096)),
            key: None,
        })
    }
    fn serialize_struct(self, _name: &'static str, len: usize) -> Result<StructSer, EncodeError> {
        Ok(StructSer(IndexMap::with_capacity(len)))
    }
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<VariantSer<StructSer>, EncodeError> {
        Ok(VariantSer {
            variant,
            inner: StructSer(IndexMap::with_capacity(len)),
        })
    }
}

pub(crate) struct SeqSer(Vec<JsValue>);

impl ser::SerializeSeq for SeqSer {
    type Ok = JsValue;
    type Error = EncodeError;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), EncodeError> {
        self.0.push(value.serialize(ValueSerializer)?);
        Ok(())
    }
    fn end(self) -> Result<JsValue, EncodeError> {
        Ok(JsValue::Array(self.0))
    }
}

impl ser::SerializeTuple for SeqSer {
    type Ok = JsValue;
    type Error = EncodeError;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), EncodeError> {
        ser::SerializeSeq::serialize_element(self, value)
    }
    fn end(self) -> Result<JsValue, EncodeError> {
        ser::SerializeSeq::end(self)
    }
}

pub(crate) struct TupleStructSer {
    tagged: bool,
    items: Vec<JsValue>,
}

impl ser::SerializeTupleStruct for TupleStructSer {
    type Ok = JsValue;
    type Error = EncodeError;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), EncodeError> {
        self.items.push(value.serialize(ValueSerializer)?);
        Ok(())
    }
    fn end(mut self) -> Result<JsValue, EncodeError> {
        if !self.tagged {
            return Ok(JsValue::Array(self.items));
        }
        let inner = self.items.pop();
        match (self.items.pop(), inner, self.items.is_empty()) {
            (Some(JsValue::Int(tag)), Some(inner), true) => u64::try_from(tag)
                .map(|tag| JsValue::Tagged(tag, Box::new(inner)))
                .map_err(|_| err("tag out of range")),
            _ => Err(err("tagged value must be (tag, value)")),
        }
    }
}

pub(crate) struct VariantSer<S> {
    variant: &'static str,
    inner: S,
}

impl ser::SerializeTupleVariant for VariantSer<SeqSer> {
    type Ok = JsValue;
    type Error = EncodeError;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), EncodeError> {
        ser::SerializeSeq::serialize_element(&mut self.inner, value)
    }
    fn end(self) -> Result<JsValue, EncodeError> {
        let mut object = IndexMap::new();
        object.insert(self.variant.to_owned(), JsValue::Array(self.inner.0));
        Ok(JsValue::Object(object))
    }
}

impl ser::SerializeStructVariant for VariantSer<StructSer> {
    type Ok = JsValue;
    type Error = EncodeError;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), EncodeError> {
        ser::SerializeStruct::serialize_field(&mut self.inner, key, value)
    }
    fn end(self) -> Result<JsValue, EncodeError> {
        let mut object = IndexMap::new();
        object.insert(self.variant.to_owned(), JsValue::Object(self.inner.0));
        Ok(JsValue::Object(object))
    }
}

pub(crate) struct MapSer {
    entries: Vec<(JsValue, JsValue)>,
    key: Option<JsValue>,
}

impl ser::SerializeMap for MapSer {
    type Ok = JsValue;
    type Error = EncodeError;
    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), EncodeError> {
        self.key = Some(key.serialize(ValueSerializer)?);
        Ok(())
    }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), EncodeError> {
        let key = self
            .key
            .take()
            .ok_or_else(|| err("map value without a key"))?;
        // Duplicate text keys collapse in `finish_map` (first position, last
        // value), like JS property assignment.
        self.entries.push((key, value.serialize(ValueSerializer)?));
        Ok(())
    }
    fn end(self) -> Result<JsValue, EncodeError> {
        Ok(finish_map(self.entries))
    }
}

pub(crate) struct StructSer(IndexMap<String, JsValue>);

impl ser::SerializeStruct for StructSer {
    type Ok = JsValue;
    type Error = EncodeError;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), EncodeError> {
        self.0
            .insert(key.to_owned(), value.serialize(ValueSerializer)?);
        Ok(())
    }
    fn end(self) -> Result<JsValue, EncodeError> {
        Ok(JsValue::Object(self.0))
    }
}
