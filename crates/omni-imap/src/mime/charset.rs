//! Charset normalization and decoding (libmime `charset.js`, mailparser body decoding).
//!
//! Decoding uses mail-parser's WHATWG-style charset table, which maps the same
//! aliases libmime/iconv-lite do (`latin1`, `ascii` and `iso-8859-1` decode as
//! windows-1252). Unknown charsets fall back to lossy UTF-8, as libmime does
//! when iconv-lite throws.

use mail_parser::decoders::charsets::map::charset_decoder;

/// libmime `normalizeCharset` for the cases that decide whether adjacent
/// encoded-words share a charset. Exact names only matter for equality.
pub(crate) fn normalize_charset(charset: &str) -> String {
    let lower = charset.trim().to_ascii_lowercase();
    let lower = if lower.is_empty() {
        "utf-8".to_owned()
    } else {
        lower
    };
    let simple = lower.replace(['-', '_'], "");
    match simple.as_str() {
        "utf8" => "UTF-8".to_owned(),
        "usascii" | "ascii" | "latin1" | "iso88591" | "win1252" | "windows1252" | "cp1252" => {
            "windows-1252".to_owned()
        }
        _ => lower.to_ascii_uppercase(),
    }
}

/// libmime `charset.decode`: UTF-8 for ascii/utf-8/7bit labels, else the
/// named charset, else lossy UTF-8.
pub(crate) fn decode_header_bytes(bytes: &[u8], charset: &str) -> String {
    let normalized = normalize_charset(charset);
    let lower = normalized.to_ascii_lowercase();
    if lower.starts_with("ascii")
        || lower.starts_with("us-ascii")
        || lower.contains("utf-8")
        || lower.ends_with("7bit")
    {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    decode_with(bytes, charset)
}

/// mailparser text-part decoding: ascii/usascii/utf8 labels (ignoring
/// punctuation) are read as UTF-8; anything else goes through the charset
/// table; unknown labels stay UTF-8.
pub(crate) fn decode_body(bytes: &[u8], charset: Option<&str>) -> String {
    let label = charset.unwrap_or("utf-8");
    let compact: String = label
        .to_ascii_lowercase()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect();
    if matches!(compact.as_str(), "ascii" | "usascii" | "utf8") {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    decode_with(bytes, label)
}

fn decode_with(bytes: &[u8], charset: &str) -> String {
    let trimmed = charset.trim();
    match charset_decoder(trimmed.as_bytes()) {
        Some(decoder) => decoder(bytes),
        None => String::from_utf8_lossy(bytes).into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_latin1_as_windows_1252() {
        assert_eq!(decode_header_bytes(&[0xe9], "iso-8859-1"), "é");
        assert_eq!(decode_body(&[0x80], Some("latin1")), "€");
        assert_eq!(decode_body("é".as_bytes(), Some("us-ascii")), "é");
        assert_eq!(decode_body(&[0xff], Some("x-unknown")), "\u{fffd}");
    }

    #[test]
    fn normalizes_for_join_equality() {
        assert_eq!(normalize_charset("utf8"), normalize_charset("UTF-8"));
        assert_eq!(normalize_charset("ascii"), "windows-1252");
    }
}
