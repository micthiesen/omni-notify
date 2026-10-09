//! `linesGz` = base64(gzip(`JSON.stringify(TaskRunLogLine[])`)) (section 4.4).

use std::borrow::Cow;
use std::io::{Read as _, Write as _};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use omni_core::LogLevel;
use serde::{Deserialize, Serialize};

use crate::StoreError;

/// `TaskRunLogLine`: one captured log line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogLine {
    /// Epoch ms of the log call.
    pub t: i64,
    pub level: LogLevel,
    /// Logger name, e.g. `"Main:LiveCheck"`.
    pub logger: String,
    pub msg: String,
}

/// Compresses lines the way `saveRunLogs` does.
pub fn encode(lines: &[LogLine]) -> Result<String, StoreError> {
    let value = serde_json::to_value(lines)
        .map_err(|e| StoreError::Sqlite(format!("serialize run log lines: {e}")))?;
    let json = omni_core::js::json_stringify(&value);
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(json.as_bytes())
        .map_err(|e| StoreError::Sqlite(format!("gzip run log lines: {e}")))?;
    let compressed = encoder
        .finish()
        .map_err(|e| StoreError::Sqlite(format!("gzip run log lines: {e}")))?;
    Ok(STANDARD.encode(compressed))
}

/// Inverse of [`encode`]; accepts any valid gzip stream.
pub fn decode(s: &str) -> Result<Vec<LogLine>, StoreError> {
    let corrupt = |reason: String| StoreError::CorruptRow {
        pk: "linesGz".to_owned(),
        reason,
    };
    let compressed = STANDARD
        .decode(s)
        .map_err(|e| corrupt(format!("invalid base64: {e}")))?;
    let mut json = String::new();
    GzDecoder::new(compressed.as_slice())
        .read_to_string(&mut json)
        .map_err(|e| corrupt(format!("invalid gzip: {e}")))?;
    serde_json::from_str(&replace_lone_surrogate_escapes(&json))
        .map_err(|e| corrupt(format!("invalid lines JSON: {e}")))
}

/// `JSON.stringify` writes a lone UTF-16 surrogate (a line truncated inside a
/// surrogate pair) as a `\udXXX` escape that serde_json rejects. Such escapes
/// become `\ufffd`, which is what node's UTF-8 conversion would show.
fn replace_lone_surrogate_escapes(json: &str) -> Cow<'_, str> {
    let bytes = json.as_bytes();
    let surrogate_at = |i: usize| -> Option<u16> {
        let hex = json.get(i + 2..i + 6)?;
        if bytes.get(i) != Some(&b'\\') || bytes.get(i + 1) != Some(&b'u') {
            return None;
        }
        u16::from_str_radix(hex, 16)
            .ok()
            .filter(|unit| (0xD800..=0xDFFF).contains(unit))
    };
    let mut out: Option<String> = None;
    let mut copied = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'\\' {
            i += 1;
            continue;
        }
        let Some(unit) = surrogate_at(i) else {
            // Skip the escaped character so `\\u...` is not misread.
            i += 2;
            continue;
        };
        let paired = (0xD800..=0xDBFF).contains(&unit)
            && surrogate_at(i + 6).is_some_and(|next| (0xDC00..=0xDFFF).contains(&next));
        if paired {
            i += 12;
            continue;
        }
        let buffer = out.get_or_insert_with(|| String::with_capacity(json.len()));
        buffer.push_str(&json[copied..i]);
        buffer.push_str("\\ufffd");
        i += 6;
        copied = i;
    }
    match out {
        Some(mut buffer) => {
            buffer.push_str(&json[copied..]);
            Cow::Owned(buffer)
        }
        None => Cow::Borrowed(json),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let lines = vec![LogLine {
            t: 1_760_000_000_123,
            level: LogLevel::Warn,
            logger: "Main:LiveCheck".to_owned(),
            msg: "hello \"world\"".to_owned(),
        }];
        let encoded = encode(&lines).ok();
        assert_eq!(
            encoded.as_deref().map(decode).and_then(Result::ok),
            Some(lines)
        );
    }

    #[test]
    fn lone_surrogate_escapes_become_replacement_characters() {
        let json = r#"["a\ud83d","\ud83d\ude00","\\ud83d","\udc00x"]"#;
        assert_eq!(
            replace_lone_surrogate_escapes(json),
            r#"["a\ufffd","\ud83d\ude00","\\ud83d","\ufffdx"]"#
        );
    }

    #[test]
    fn decodes_node_output() {
        // node: zlib.gzipSync(JSON.stringify([{t:1,level:"info",logger:"L",msg:"m"}])).toString("base64")
        let from_node = "H4sIAAAAAAAAE4uuVipRsjLUUcpJLUvNUbJSysxLy1fSUcrJT09PLVKyUvJR0lHKLU5XslLKVaqNBQD3d9x/LwAAAA==";
        let lines = decode(from_node).ok();
        assert_eq!(
            lines,
            Some(vec![LogLine {
                t: 1,
                level: LogLevel::Info,
                logger: "L".to_owned(),
                msg: "m".to_owned()
            }])
        );
    }
}
