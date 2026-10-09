//! Persisted documents that keep unknown top-level fields.
//!
//! TS writes these states with explicit `undefined` properties (for example
//! `error: undefined`), which serde's buffered `#[serde(flatten)]` path cannot
//! read into typed optional fields. [`Stored`] instead splits the object into
//! the known fields (decoded unbuffered, so `undefined` reads as `None`) and
//! the rest, which is written back unchanged.

use omni_store::cbor::{self, Extra, JsValue};
use serde::de::{DeserializeOwned, Error as _};
use serde::ser::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A document type with a fixed set of top-level property names.
pub trait StoredFields: Serialize + DeserializeOwned {
    /// Every serialized property name of the type.
    const FIELDS: &'static [&'static str];
}

/// A typed document plus the unknown properties read with it.
#[derive(Clone, Debug, PartialEq)]
pub struct Stored<T> {
    pub value: T,
    pub extra: Extra,
}

impl<T> Stored<T> {
    pub fn new(value: T) -> Self {
        Self {
            value,
            extra: Extra::new(),
        }
    }
}

impl<T: StoredFields> Serialize for Stored<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut value = cbor::to_value(&self.value).map_err(S::Error::custom)?;
        let Some(object) = value.as_object_mut() else {
            return Err(S::Error::custom("stored document is not an object"));
        };
        for (key, extra) in &self.extra {
            if !object.contains_key(key) {
                object.insert(key.clone(), extra.clone());
            }
        }
        value.serialize(serializer)
    }
}

impl<'de, T: StoredFields> Deserialize<'de> for Stored<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let JsValue::Object(object) = JsValue::deserialize(deserializer)? else {
            return Err(D::Error::custom("stored document is not an object"));
        };
        let mut known = Extra::new();
        let mut extra = Extra::new();
        for (key, value) in object {
            if T::FIELDS.contains(&key.as_str()) {
                known.insert(key, value);
            } else {
                extra.insert(key, value);
            }
        }
        let value = cbor::from_value(JsValue::Object(known)).map_err(D::Error::custom)?;
        Ok(Self { value, extra })
    }
}
