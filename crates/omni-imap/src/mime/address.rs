//! Address headers: a port of nodemailer `addressparser` plus mailparser's
//! `decodeAddresses` (encoded-word names, encoded-word address rejection,
//! punycode domains).

use std::sync::LazyLock;

use regex::Regex;

use super::words::decode_words;
use omni_core::js::trim as js_trim;

/// One parsed address; a group has `group` members and no address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Address {
    pub name: String,
    pub address: Option<String>,
    pub group: Option<Vec<Address>>,
}

#[derive(Clone, Debug)]
struct Token {
    operator: bool,
    value: String,
    no_break: bool,
}

const MAX_NESTED_GROUP_DEPTH: usize = 50;

fn operator_end(c: char) -> Option<char> {
    match c {
        '"' => Some('"'),
        '(' => Some(')'),
        '<' => Some('>'),
        ',' | ';' => Some('\0'),
        ':' => Some(';'),
        _ => None,
    }
}

fn tokenize(input: &str) -> Vec<Token> {
    let chars: Vec<char> = input.chars().collect();
    let mut list: Vec<Token> = Vec::new();
    let mut current: Option<usize> = None;
    let mut expecting: Option<char> = None;
    let mut escaped = false;
    let mut in_domain_literal = false;

    for (i, &c) in chars.iter().enumerate() {
        let next = chars.get(i + 1).copied();
        if !escaped && expecting.is_none_or(|e| e == '\0') {
            if !in_domain_literal && c == '[' {
                in_domain_literal = true;
            } else if in_domain_literal && matches!(c, ']' | ',' | ';') {
                in_domain_literal = false;
            }
        }
        let expecting_char = expecting.filter(|e| *e != '\0');
        if escaped {
            // falls through to text handling
        } else if Some(c) == expecting_char {
            let no_break = next.is_some_and(|n| !matches!(n, ' ' | '\t' | '\r' | '\n' | ',' | ';'));
            list.push(Token {
                operator: true,
                value: c.to_string(),
                no_break,
            });
            current = None;
            expecting = None;
            escaped = false;
            continue;
        } else if expecting_char.is_none() && !in_domain_literal && operator_end(c).is_some() {
            list.push(Token {
                operator: true,
                value: c.to_string(),
                no_break: false,
            });
            current = None;
            expecting = operator_end(c).filter(|e| *e != '\0');
            escaped = false;
            continue;
        } else if matches!(expecting_char, Some('"') | Some('\'')) && c == '\\' {
            escaped = true;
            continue;
        }

        let index = match current {
            Some(index) => index,
            None => {
                list.push(Token {
                    operator: false,
                    value: String::new(),
                    no_break: false,
                });
                let index = list.len() - 1;
                current = Some(index);
                index
            }
        };
        let c = if c == '\n' { ' ' } else { c };
        if (c as u32) >= 0x21 || c == ' ' || c == '\t' {
            list[index].value.push(c);
        }
        escaped = false;
    }

    list.into_iter()
        .filter_map(|mut token| {
            token.value = js_trim(&token.value).to_owned();
            (!token.value.is_empty()).then_some(token)
        })
        .collect()
}

static STRICT_EMAIL: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"^[^@\s]+@[^@\s]+$").ok());
static LOOSE_EMAIL: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"\s*(?-u:\b)[^@\s]+@[^\s]+(?-u:\b)\s*").ok());
static ADDRESS_PREFIX: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"^[^<]*<\s*").ok());

fn handle_address(tokens: &[Token], depth: usize) -> Vec<Address> {
    #[derive(Clone, Copy, PartialEq)]
    enum State {
        Text,
        Address,
        Comment,
        Group,
    }
    let mut is_group = false;
    let mut state = State::Text;
    let mut address: Vec<String> = Vec::new();
    let mut comment: Vec<String> = Vec::new();
    let mut group: Vec<String> = Vec::new();
    let mut text: Vec<String> = Vec::new();
    let mut text_quoted: Vec<bool> = Vec::new();
    let mut inside_quotes = false;

    for (i, token) in tokens.iter().enumerate() {
        let prev = if i > 0 { tokens.get(i - 1) } else { None };
        if token.operator {
            match token.value.as_str() {
                "<" => {
                    state = State::Address;
                    inside_quotes = false;
                }
                "(" => {
                    state = State::Comment;
                    inside_quotes = false;
                }
                ":" => {
                    state = State::Group;
                    is_group = true;
                    inside_quotes = false;
                }
                "\"" => {
                    inside_quotes = !inside_quotes;
                    state = State::Text;
                }
                _ => {
                    state = State::Text;
                    inside_quotes = false;
                }
            }
            continue;
        }
        let mut value = token.value.clone();
        if state == State::Address
            && let Some(re) = ADDRESS_PREFIX.as_ref()
        {
            value = re.replace(&value, "").into_owned();
        }
        let target = match state {
            State::Text => &mut text,
            State::Address => &mut address,
            State::Comment => &mut comment,
            State::Group => &mut group,
        };
        if prev.is_some_and(|p| p.no_break) && !target.is_empty() {
            if let Some(last) = target.last_mut() {
                last.push_str(&value);
            }
            if state == State::Text
                && inside_quotes
                && let Some(last) = text_quoted.last_mut()
            {
                *last = true;
            }
        } else {
            target.push(value);
            if state == State::Text {
                text_quoted.push(inside_quotes);
            }
        }
    }

    if text.is_empty() && !comment.is_empty() {
        text = std::mem::take(&mut comment);
    }

    if is_group {
        let name = text.join(" ");
        let mut members = Vec::new();
        if !group.is_empty() {
            for member in parse_depth(&group.join(","), depth + 1) {
                match member.group {
                    Some(nested) => members.extend(nested),
                    None => members.push(member),
                }
            }
        }
        return vec![Address {
            name,
            address: None,
            group: Some(members),
        }];
    }

    if address.is_empty() && !text.is_empty() {
        for i in (0..text.len()).rev() {
            let quoted = text_quoted.get(i).copied().unwrap_or(false);
            if !quoted
                && STRICT_EMAIL
                    .as_ref()
                    .is_some_and(|re| re.is_match(&text[i]))
            {
                address = vec![text.remove(i)];
                if i < text_quoted.len() {
                    text_quoted.remove(i);
                }
                break;
            }
        }
        if address.is_empty() {
            for i in (0..text.len()).rev() {
                if text_quoted.get(i).copied().unwrap_or(false) {
                    continue;
                }
                let Some(re) = LOOSE_EMAIL.as_ref() else {
                    break;
                };
                if let Some(found) = re.find(&text[i]) {
                    address = vec![js_trim(found.as_str()).to_owned()];
                    let replaced =
                        format!("{} {}", &text[i][..found.start()], &text[i][found.end()..]);
                    text[i] = js_trim(&replaced).to_owned();
                    break;
                }
                text[i] = js_trim(&text[i]).to_owned();
            }
        }
    }

    if text.is_empty() && !comment.is_empty() {
        text = std::mem::take(&mut comment);
    }
    if address.len() > 1 {
        let extra: Vec<String> = address.drain(1..).collect();
        text.extend(extra);
    }
    let text = text.join(" ");
    let addr = address.join(" ");
    let mut out_address = if addr.is_empty() {
        text.clone()
    } else {
        addr.clone()
    };
    let mut out_name = if text.is_empty() { addr } else { text };
    if out_address == out_name {
        if out_address.contains('@') {
            out_name = String::new();
        } else {
            out_address = String::new();
        }
    }
    vec![Address {
        name: out_name,
        address: Some(out_address),
        group: None,
    }]
}

fn parse_depth(input: &str, depth: usize) -> Vec<Address> {
    if depth > MAX_NESTED_GROUP_DEPTH {
        return Vec::new();
    }
    let tokens = tokenize(input);
    let mut lists: Vec<Vec<Token>> = Vec::new();
    let mut current: Vec<Token> = Vec::new();
    for token in tokens {
        if token.operator && (token.value == "," || token.value == ";") {
            if !current.is_empty() {
                lists.push(std::mem::take(&mut current));
            }
        } else {
            current.push(token);
        }
    }
    if !current.is_empty() {
        lists.push(current);
    }
    let mut parsed: Vec<Address> = lists
        .iter()
        .flat_map(|tokens| handle_address(tokens, depth))
        .collect();
    // Merge "Joe Foo, PhD <joe@example.com>" fragments.
    let mut i = parsed.len().saturating_sub(1);
    while i > 0 {
        i -= 1;
        let mergeable = {
            let current = &parsed[i];
            let next = &parsed[i + 1];
            current.address.as_deref() == Some("")
                && !current.name.is_empty()
                && current.group.is_none()
                && next.address.as_deref().is_some_and(|a| !a.is_empty())
                && !next.name.is_empty()
        };
        if mergeable {
            let name = format!("{}, {}", parsed[i].name, parsed[i + 1].name);
            parsed[i + 1].name = name;
            parsed.remove(i);
        }
    }
    parsed
}

/// nodemailer `addressparser(str)`.
pub fn parse_addresses(input: &str) -> Vec<Address> {
    parse_depth(input, 0)
}

static B_WORDS_ONLY: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(r"^(=\?([^?]+)\?[Bb]\?[^?]*\?=)(\s*=\?([^?]+)\?[Bb]\?[^?]*\?=)*$").ok()
});
static BRACKETED: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"<[^<>]+@[^<>]+>").ok());
static ENCODED_IN_ADDRESS: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"[=]\?[^?]+\?[BbQq]\?[^?]*\?[=]").ok());
static SIMPLE_ADDRESS: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"^[^\s@]+@[^\s@]+$").ok());

fn matches(re: &LazyLock<Option<Regex>>, s: &str) -> bool {
    re.as_ref().is_some_and(|re| re.is_match(s))
}

/// mailparser `decodeAddresses`.
pub(crate) fn decode_addresses(addresses: &mut Vec<Address>) {
    let mut processed: Vec<Address> = Vec::new();
    let mut i = 0;
    while i < addresses.len() {
        let already = processed.iter().any(|p| p == &addresses[i]);
        let address = &mut addresses[i];
        address.name = js_trim(&address.name).to_owned();
        if address.address.as_deref().is_none_or(str::is_empty)
            && matches(&B_WORDS_ONLY, &address.name)
            && !already
        {
            let decoded = decode_words(&address.name);
            if matches(&BRACKETED, &decoded) {
                let parsed = parse_addresses(&decoded);
                addresses.remove(i);
                for entry in parsed {
                    processed.push(entry.clone());
                    addresses.push(entry);
                }
                continue;
            }
            address.name = decoded;
            i += 1;
            continue;
        }
        if !address.name.is_empty() {
            address.name = decode_words(&address.name);
        }
        if let Some(addr) = address.address.clone()
            && matches(&ENCODED_IN_ADDRESS, &addr)
        {
            let decoded = decode_words(&addr);
            address.address = Some(
                if matches(&SIMPLE_ADDRESS, &decoded) && !decoded.contains("=?") {
                    decoded
                } else {
                    String::new()
                },
            );
        }
        if let Some(addr) = address.address.clone()
            && addr.contains("@xn--")
            && let Some(at) = addr.rfind('@')
        {
            let domain = &addr[at + 1..];
            if let Some(unicode) = punycode_domain_to_unicode(domain) {
                address.address = Some(format!("{}{}", &addr[..=at], unicode));
            }
        }
        if let Some(group) = address.group.as_mut() {
            decode_addresses(group);
        }
        i += 1;
    }
}

/// `punycode.toUnicode` for a domain: each `xn--` label decoded (RFC 3492).
fn punycode_domain_to_unicode(domain: &str) -> Option<String> {
    let labels: Vec<String> = domain
        .split(['.', '\u{3002}', '\u{ff0e}', '\u{ff61}'])
        .map(|label| {
            if label.len() > 4 && label[..4].eq_ignore_ascii_case("xn--") {
                punycode_decode(&label[4..].to_ascii_lowercase())
            } else {
                Some(label.to_owned())
            }
        })
        .collect::<Option<Vec<_>>>()?;
    Some(labels.join("."))
}

fn punycode_decode(input: &str) -> Option<String> {
    const BASE: u32 = 36;
    const TMIN: u32 = 1;
    const TMAX: u32 = 26;
    const SKEW: u32 = 38;
    const DAMP: u32 = 700;
    let (basic, encoded) = match input.rfind('-') {
        Some(idx) => (&input[..idx], &input[idx + 1..]),
        None => ("", input),
    };
    let mut output: Vec<char> = basic.chars().collect();
    if output.iter().any(|c| !c.is_ascii()) {
        return None;
    }
    let mut n: u32 = 128;
    let mut i: u32 = 0;
    let mut bias: u32 = 72;
    let bytes = encoded.as_bytes();
    let mut pos = 0;
    while pos < bytes.len() {
        let old_i = i;
        let mut w: u32 = 1;
        let mut k = BASE;
        loop {
            let byte = *bytes.get(pos)?;
            pos += 1;
            let digit = match byte {
                b'a'..=b'z' => u32::from(byte - b'a'),
                b'0'..=b'9' => u32::from(byte - b'0') + 26,
                _ => return None,
            };
            i = i.checked_add(digit.checked_mul(w)?)?;
            let t = if k <= bias {
                TMIN
            } else if k >= bias + TMAX {
                TMAX
            } else {
                k - bias
            };
            if digit < t {
                break;
            }
            w = w.checked_mul(BASE - t)?;
            k += BASE;
        }
        let len = u32::try_from(output.len()).ok()? + 1;
        // adapt
        let mut delta = if old_i == 0 {
            (i - old_i) / DAMP
        } else {
            (i - old_i) / 2
        };
        delta += delta / len;
        let mut k = 0;
        while delta > ((BASE - TMIN) * TMAX) / 2 {
            delta /= BASE - TMIN;
            k += BASE;
        }
        bias = k + (BASE - TMIN + 1) * delta / (delta + SKEW);
        n = n.checked_add(i / len)?;
        i %= len;
        let c = char::from_u32(n)?;
        output.insert(usize::try_from(i).ok()?, c);
        i += 1;
    }
    Some(output.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(name: &str, address: &str) -> Address {
        Address {
            name: name.to_owned(),
            address: Some(address.to_owned()),
            group: None,
        }
    }

    #[test]
    fn parses_common_forms() {
        assert_eq!(
            parse_addresses("Reader <reader@example.test>, other@example.test"),
            vec![
                plain("Reader", "reader@example.test"),
                plain("", "other@example.test")
            ]
        );
        assert_eq!(
            parse_addresses("\"Doe, Jane\" <jane@example.test>"),
            vec![plain("Doe, Jane", "jane@example.test")]
        );
        assert_eq!(
            parse_addresses("Joe Foo, PhD <joe@example.com>"),
            vec![plain("Joe Foo, PhD", "joe@example.com")]
        );
        assert_eq!(
            parse_addresses("jane@example.test (Jane)"),
            vec![plain("Jane", "jane@example.test")]
        );
    }

    #[test]
    fn parses_groups() {
        let parsed = parse_addresses("Team: a@example.test, b@example.test;");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "Team");
        assert_eq!(parsed[0].group.as_ref().map(Vec::len), Some(2));
    }

    #[test]
    fn decodes_names_and_punycode() {
        let mut parsed = parse_addresses("=?utf-8?Q?Ren=C3=A9?= <rene@xn--bcher-kva.example>");
        decode_addresses(&mut parsed);
        assert_eq!(parsed, vec![plain("René", "rene@bücher.example")]);
    }
}
