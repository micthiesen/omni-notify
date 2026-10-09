//! RFC 2047 encoded-word decoding with libmime `decodeWords` semantics:
//! adjacent B or Q words in the same charset are joined before decoding (so a
//! character split across words survives), whitespace between encoded words is
//! dropped, and malformed words stay literal.

use super::charset::{decode_header_bytes, normalize_charset};
use super::transfer::node_base64;

#[derive(Debug)]
struct Word {
    start: usize,
    end: usize,
    charset: String,
    encoding: u8,
    text: String,
}

/// Finds `=?<charset>?<B|Q>?<text>?=` left to right, non-overlapping, where
/// the charset is `[^?]+` and the text `[^?]*` (the libmime join pattern).
fn find_words(s: &str) -> Vec<Word> {
    let bytes = s.as_bytes();
    let mut words = Vec::new();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'='
            && bytes[i + 1] == b'?'
            && let Some(word) = match_word_at(s, i)
        {
            i = word.end;
            words.push(word);
            continue;
        }
        i += 1;
    }
    words
}

fn match_word_at(s: &str, start: usize) -> Option<Word> {
    let bytes = s.as_bytes();
    let charset_start = start + 2;
    let charset_end = charset_start
        + bytes
            .get(charset_start..)?
            .iter()
            .position(|&b| b == b'?')?;
    if charset_end == charset_start {
        return None;
    }
    let encoding = *bytes.get(charset_end + 1)?;
    if !matches!(encoding, b'B' | b'b' | b'Q' | b'q') || bytes.get(charset_end + 2) != Some(&b'?') {
        return None;
    }
    let text_start = charset_end + 3;
    let text_end = text_start + bytes.get(text_start..)?.iter().position(|&b| b == b'?')?;
    if bytes.get(text_end + 1) != Some(&b'=') {
        return None;
    }
    Some(Word {
        start,
        end: text_end + 2,
        charset: s[charset_start..charset_end].to_owned(),
        encoding: encoding.to_ascii_uppercase(),
        text: s[text_start..text_end].to_owned(),
    })
}

fn is_js_space(c: char) -> bool {
    c.is_whitespace() || c == '\u{feff}'
}

/// libmime `decodeWords`.
pub(crate) fn decode_words(s: &str) -> String {
    let words = find_words(s);
    if words.is_empty() {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut cursor = 0;
    let mut i = 0;
    while i < words.len() {
        out.push_str(&s[cursor..words[i].start]);
        // Group joinable neighbours (only whitespace between, same encoding and charset).
        let mut charset = words[i].charset.clone();
        let encoding = words[i].encoding;
        let mut text = words[i].text.clone();
        let mut end = words[i].end;
        let mut j = i + 1;
        while j < words.len() {
            let gap = &s[end..words[j].start];
            if !gap.chars().all(is_js_space) {
                break;
            }
            let next = &words[j];
            if next.encoding == encoding
                && normalize_charset(&next.charset) == normalize_charset(&charset)
            {
                text.push_str(&next.text);
                end = next.end;
                j += 1;
                continue;
            }
            break;
        }
        out.push_str(&decode_one(
            &charset,
            encoding,
            &text,
            &s[words[i].start..end],
        ));
        cursor = end;
        // Whitespace between this group and a following encoded word is removed.
        if j < words.len() && s[end..words[j].start].chars().all(is_js_space) {
            cursor = words[j].start;
        }
        charset.clear();
        i = j;
    }
    out.push_str(&s[cursor..]);
    out
}

/// Decodes one (possibly joined) word if its charset matches `[\w_\-*]+`,
/// else returns the original text unchanged.
fn decode_one(charset: &str, encoding: u8, text: &str, original: &str) -> String {
    let valid = !charset.is_empty()
        && charset
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'*'));
    if !valid {
        return original.to_owned();
    }
    decode_word(charset, encoding, text)
}

/// libmime `decodeWord`.
pub(crate) fn decode_word(charset: &str, encoding: u8, text: &str) -> String {
    let charset = charset.split('*').next().unwrap_or(charset);
    let bytes: Vec<u8> = match encoding {
        b'Q' => decode_q(text),
        b'B' => text
            .split('=')
            .filter(|piece| !piece.is_empty())
            .flat_map(|piece| node_base64(piece.as_bytes()))
            .collect(),
        _ => text.as_bytes().to_vec(),
    };
    decode_header_bytes(&bytes, charset)
}

fn decode_q(text: &str) -> Vec<u8> {
    // `.replace(/=\s+([0-9a-fA-F])/g, "=$1").replace(/[_\s]/g, " ")`
    let mut cleaned = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '=' {
            let mut k = i + 1;
            while k < chars.len() && is_js_space(chars[k]) {
                k += 1;
            }
            if k > i + 1 && k < chars.len() && chars[k].is_ascii_hexdigit() {
                cleaned.push('=');
                i = k;
                continue;
            }
        }
        cleaned.push(if chars[i] == '_' || is_js_space(chars[i]) {
            ' '
        } else {
            chars[i]
        });
        i += 1;
    }
    let buf = cleaned.as_bytes();
    let mut out = Vec::with_capacity(buf.len());
    let mut i = 0;
    while i < buf.len() {
        let c = buf[i];
        if c == b'='
            && let (Some(&a), Some(&b)) = (buf.get(i + 1), buf.get(i + 2))
            && let (Some(high), Some(low)) = (hex_value(a), hex_value(b))
        {
            out.push((high << 4) | low);
            i += 3;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

fn hex_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_and_joins_words() {
        assert_eq!(decode_words("=?UTF-8?Q?Caf=C3=A9?="), "Café");
        assert_eq!(decode_words("=?utf-8?B?w6k=?= =?UTF-8?B?w6k=?="), "éé");
        // A multi-byte character split across two Q words.
        assert_eq!(decode_words("=?utf-8?Q?=C3?= =?utf-8?Q?=A9?="), "é");
        assert_eq!(decode_words("Re: =?utf-8?Q?a_b?= tail"), "Re: a b tail");
        assert_eq!(decode_words("=?bad charset?Q?x?="), "=?bad charset?Q?x?=");
        assert_eq!(decode_words("plain"), "plain");
    }

    #[test]
    fn mixed_encodings_drop_the_gap_only() {
        assert_eq!(decode_words("=?utf-8?B?YQ==?= =?utf-8?Q?b?="), "ab");
    }
}
