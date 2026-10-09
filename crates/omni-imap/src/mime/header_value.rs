//! Structured header values (libmime `parseHeaderValue`, `decodeHeader`) with
//! RFC 2231 parameter continuations and charsets.

use indexmap::IndexMap;

use super::words::decode_words;
pub(crate) use omni_core::js::trim as js_trim;

/// `{ value, params }`; `value` is `None` where libmime leaves `false`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct StructuredHeader {
    pub value: Option<String>,
    pub params: IndexMap<String, String>,
}

#[derive(Clone, Debug)]
enum Param {
    Plain(String),
    Continued {
        charset: Option<String>,
        values: Vec<(u64, String)>,
    },
}

/// libmime `parseHeaderValue`.
pub(crate) fn parse_header_value(input: &str) -> StructuredHeader {
    #[derive(PartialEq)]
    enum Stage {
        Key,
        Value,
    }
    let mut value_part: Option<String> = None;
    let mut params: IndexMap<String, Param> = IndexMap::new();
    let mut key: Option<String> = None;
    let mut buf = String::new();
    let mut stage = Stage::Value;
    let mut quote: Option<char> = None;
    let mut escaped = false;

    for chr in input.chars() {
        match stage {
            Stage::Key => {
                if chr == '=' {
                    key = Some(js_trim(&buf).to_lowercase());
                    stage = Stage::Value;
                    buf.clear();
                } else {
                    buf.push(chr);
                }
            }
            Stage::Value => {
                if escaped {
                    buf.push(chr);
                } else if chr == '\\' {
                    escaped = true;
                    continue;
                } else if quote == Some(chr) {
                    quote = None;
                } else if quote.is_none() && chr == '"' {
                    quote = Some(chr);
                } else if quote.is_none() && chr == ';' {
                    match &key {
                        None => value_part = Some(js_trim(&buf).to_owned()),
                        Some(k) => {
                            params.insert(k.clone(), Param::Plain(js_trim(&buf).to_owned()));
                        }
                    }
                    stage = Stage::Key;
                    buf.clear();
                } else {
                    buf.push(chr);
                }
                escaped = false;
            }
        }
    }

    let rest = js_trim(&buf).to_owned();
    if stage == Stage::Value {
        match &key {
            None => value_part = Some(rest),
            Some(k) => {
                params.insert(k.clone(), Param::Plain(rest));
            }
        }
    } else if !rest.is_empty() {
        params.insert(rest.to_lowercase(), Param::Plain(String::new()));
    }

    // RFC 2231 continuations: `name*`, `name*0`, `name*0*`, ...
    let keys: Vec<String> = params.keys().cloned().collect();
    for k in keys {
        let Some((actual, nr, extended)) = continuation_key(&k) else {
            continue;
        };
        let Some(Param::Plain(mut raw)) = params.get(&k).cloned() else {
            continue;
        };
        let mut charset_found: Option<String> = None;
        if nr == 0
            && extended
            && let Some((charset, rest)) = split_extended(&raw)
        {
            charset_found = Some(if charset.is_empty() {
                "utf-8".to_owned()
            } else {
                charset
            });
            raw = rest;
        }
        let entry = params.get_mut(&actual);
        match entry {
            Some(Param::Continued { charset, values }) => {
                if let Some(found) = charset_found {
                    *charset = Some(found);
                }
                values.push((nr, raw));
            }
            _ => {
                params.insert(
                    actual.clone(),
                    Param::Continued {
                        charset: charset_found,
                        values: vec![(nr, raw)],
                    },
                );
            }
        }
        params.shift_remove(&k);
    }

    let mut out = IndexMap::new();
    for (k, param) in params {
        let value = match param {
            Param::Plain(v) => v,
            Param::Continued {
                charset,
                mut values,
            } => {
                values.sort_by_key(|(nr, _)| *nr);
                let joined: String = values.into_iter().map(|(_, v)| v).collect();
                match charset {
                    Some(charset) => {
                        decode_words(&format!("=?{charset}?Q?{}?=", percent_to_q(&joined)))
                    }
                    None => decode_words(&joined),
                }
            }
        };
        out.insert(k, value);
    }
    StructuredHeader {
        value: value_part,
        params: out,
    }
}

/// `key.match(/\*((\d+)\*?)?$/)`: (actual key, number, ends with `*`).
fn continuation_key(key: &str) -> Option<(String, u64, bool)> {
    let bytes = key.as_bytes();
    let mut end = bytes.len();
    let ends_with_star = end > 0 && bytes[end - 1] == b'*';
    // Form 1: `name*<digits>*` or `name*<digits>`; form 2: `name*`.
    let mut digits_end = end;
    if ends_with_star {
        digits_end = end - 1;
    }
    let mut digits_start = digits_end;
    while digits_start > 0 && bytes[digits_start - 1].is_ascii_digit() {
        digits_start -= 1;
    }
    if digits_start < digits_end && digits_start > 0 && bytes[digits_start - 1] == b'*' {
        let nr = key[digits_start..digits_end].parse::<u64>().unwrap_or(0);
        let actual = key[..digits_start - 1].to_lowercase();
        let matched_suffix_ends_with_star = ends_with_star;
        return Some((actual, nr, matched_suffix_ends_with_star));
    }
    if ends_with_star {
        end -= 1;
        return Some((key[..end].to_lowercase(), 0, true));
    }
    None
}

/// `value.match(/^([^']*)'[^']*'(.*)$/)`.
fn split_extended(value: &str) -> Option<(String, String)> {
    let first = value.find('\'')?;
    let rest = &value[first + 1..];
    let second = rest.find('\'')?;
    Some((value[..first].to_owned(), rest[second + 1..].to_owned()))
}

/// RFC 2231 percent-encoding to a Q-encoded payload.
fn percent_to_q(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            ' ' => out.push('_'),
            '=' | '?' | '_' => out.push_str(&format!("={:02x}", c as u32)),
            c if c.is_whitespace() => {
                let code = c as u32;
                out.push_str(&format!("={code:02x}"));
            }
            '%' => out.push('='),
            c => out.push(c),
        }
    }
    out
}

/// libmime `decodeHeader`: unfold, split at the first colon; key lowercased.
pub(crate) fn decode_header_line(line: &str) -> (String, String) {
    let mut unfolded = String::with_capacity(line.len());
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\r' || c == '\n' {
            if c == '\r' && chars.get(i + 1) == Some(&'\n') {
                i += 1;
            }
            i += 1;
            while i < chars.len() && (chars[i] == ' ' || chars[i] == '\t') {
                i += 1;
            }
            unfolded.push(' ');
            continue;
        }
        unfolded.push(c);
        i += 1;
    }
    let trimmed = js_trim(&unfolded);
    let leading = trimmed.trim_start_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
    match leading.find(':') {
        Some(0) | None => (String::new(), String::new()),
        Some(idx) => (
            js_trim(&leading[..idx]).to_lowercase(),
            js_trim(&leading[idx + 1..]).to_owned(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_value_and_params() {
        let parsed = parse_header_value("multipart/mixed; boundary=\"outer\"; CHARSET=UTF-8");
        assert_eq!(parsed.value.as_deref(), Some("multipart/mixed"));
        assert_eq!(
            parsed.params.get("boundary").map(String::as_str),
            Some("outer")
        );
        assert_eq!(
            parsed.params.get("charset").map(String::as_str),
            Some("UTF-8")
        );
    }

    #[test]
    fn joins_rfc2231_continuations() {
        let parsed =
            parse_header_value("attachment; filename*0*=utf-8''caf%C3%A9; filename*1*=%20menu.pdf");
        assert_eq!(
            parsed.params.get("filename").map(String::as_str),
            Some("café menu.pdf")
        );
        let plain = parse_header_value("attachment; filename*0=\"a\"; filename*1=\"b.txt\"");
        assert_eq!(
            plain.params.get("filename").map(String::as_str),
            Some("ab.txt")
        );
    }

    #[test]
    fn decodes_header_lines() {
        assert_eq!(
            decode_header_line("Subject: hello\r\n\tworld "),
            ("subject".to_owned(), "hello world".to_owned())
        );
        assert_eq!(
            decode_header_line("no colon"),
            (String::new(), String::new())
        );
    }
}
