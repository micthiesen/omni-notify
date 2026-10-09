//! Bounded public JSON reads and shared field rules.

use std::time::Duration;

use jiff::tz::TimeZone;
use omni_core::js::utf16_len;
use omni_http::public::PublicHttpClient;
use omni_http::{Method, Url};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};

use crate::js::parse_date;

/// Free text bound (`resetText`).
pub const MAX_TEXT: usize = 16_384;
/// Identifier bound (`resetIdentifier`).
pub const MAX_IDENTIFIER: usize = 256;
/// Source URL bound (`resetUrl`).
pub const MAX_URL: usize = 512;
const READ_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;

/// A source read, parse or validation failure; fails the run visibly.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{operation}: {cause}")]
pub struct ResetSourceError {
    pub operation: String,
    pub cause: String,
}

impl ResetSourceError {
    pub fn new(operation: impl Into<String>, cause: impl Into<String>) -> Self {
        Self {
            operation: operation.into(),
            cause: cause.into(),
        }
    }
}

/// Field checks shared by the provider schemas. Each returns a message naming
/// the field path on failure.
pub struct Check<'a> {
    pub tz: &'a TimeZone,
}

impl Check<'_> {
    pub fn text(&self, path: &str, value: &str) -> Result<(), String> {
        max_len(path, value, MAX_TEXT)
    }

    pub fn identifier(&self, path: &str, value: &str) -> Result<(), String> {
        max_len(path, value, MAX_IDENTIFIER)
    }

    pub fn timestamp(&self, path: &str, value: &str) -> Result<(), String> {
        parse_date(value, self.tz)
            .map(|_| ())
            .ok_or_else(|| format!("{path}: expected a valid timestamp, got {value:?}"))
    }

    pub fn url(&self, path: &str, value: &str) -> Result<(), String> {
        max_len(path, value, MAX_URL)?;
        match Url::parse(value) {
            Ok(url)
                if url.scheme() == "https"
                    && url.username().is_empty()
                    && url.password().is_none() =>
            {
                Ok(())
            }
            _ => Err(format!("{path}: expected an HTTPS URL without credentials")),
        }
    }

    pub fn max_items(&self, path: &str, len: usize, max: usize) -> Result<(), String> {
        if len > max {
            return Err(format!("{path}: expected at most {max} items, got {len}"));
        }
        Ok(())
    }
}

fn max_len(path: &str, value: &str, max: usize) -> Result<(), String> {
    let len = utf16_len(value);
    if len > max {
        return Err(format!(
            "{path}: expected at most {max} characters, got {len}"
        ));
    }
    Ok(())
}

/// A required field that may be `null` (Effect `Schema.NullOr`): missing is an error.
pub fn required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

/// An optional field that must not be `null` (`Schema.optional`); use with `default`.
pub fn optional_non_null<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

/// Decodes a JSON value into a schema type, then runs its field checks.
pub fn decode<T, F>(value: serde_json::Value, validate: F) -> Result<T, String>
where
    T: DeserializeOwned,
    F: FnOnce(&T) -> Result<(), String>,
{
    let decoded: T = serde_json::from_value(value).map_err(|e| e.to_string())?;
    validate(&decoded)?;
    Ok(decoded)
}

/// `readResetSource`: GET with a 20 s timeout and a 2 MiB cap, then JSON
/// parse and schema decode. No retries.
pub async fn read_json(
    http: &PublicHttpClient,
    url: &str,
) -> Result<serde_json::Value, ResetSourceError> {
    let read = |cause: String| ResetSourceError::new(format!("read {url}"), cause);
    let parsed = Url::parse(url).map_err(|e| read(e.to_string()))?;
    let response = http
        .request(Method::GET, parsed)
        .header("accept", "application/json")
        .timeout(READ_TIMEOUT)
        .send_bounded(MAX_BODY_BYTES)
        .await
        .map_err(|e| read(format!("fetch reset source {url} failed: {e}")))?;
    if !response.status.is_success() {
        return Err(read(format!(
            "fetch reset source {url} failed: Response code {}",
            response.status.as_u16()
        )));
    }
    serde_json::from_slice(&response.body).map_err(|e| read(format!("parse {url}: {e}")))
}
