//! IMAP modified UTF-7 mailbox names (RFC 3501 section 5.1.3).

use base64::Engine as _;
use base64::alphabet::IMAP_MUTF7;
use base64::engine::{GeneralPurpose, GeneralPurposeConfig};

const ENGINE: GeneralPurpose = GeneralPurpose::new(
    &IMAP_MUTF7,
    GeneralPurposeConfig::new()
        .with_encode_padding(false)
        .with_decode_padding_mode(base64::engine::DecodePaddingMode::Indifferent),
);

/// Encodes a mailbox name for the wire.
pub fn encode(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut pending: Vec<u16> = Vec::new();
    let flush = |pending: &mut Vec<u16>, out: &mut String| {
        if pending.is_empty() {
            return;
        }
        let bytes: Vec<u8> = pending.iter().flat_map(|u| u.to_be_bytes()).collect();
        out.push('&');
        out.push_str(&ENGINE.encode(bytes));
        out.push('-');
        pending.clear();
    };
    for c in name.chars() {
        if (' '..='~').contains(&c) {
            flush(&mut pending, &mut out);
            if c == '&' {
                out.push_str("&-");
            } else {
                out.push(c);
            }
        } else {
            let mut buf = [0u16; 2];
            pending.extend_from_slice(c.encode_utf16(&mut buf));
        }
    }
    flush(&mut pending, &mut out);
    out
}

/// Decodes a wire mailbox name; malformed sequences are kept literally.
pub fn decode(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut rest = name;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('-') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let encoded = &after[..end];
        if encoded.is_empty() {
            out.push('&');
        } else {
            let decoded = ENGINE
                .decode(encoded)
                .ok()
                .filter(|bytes| bytes.len() % 2 == 0)
                .and_then(|bytes| {
                    let units: Vec<u16> = bytes
                        .chunks_exact(2)
                        .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
                        .collect();
                    String::from_utf16(&units).ok()
                });
            match decoded {
                Some(text) => out.push_str(&text),
                None => {
                    out.push('&');
                    out.push_str(encoded);
                    out.push('-');
                }
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_localized_names() {
        assert_eq!(encode("Courrier envoyé"), "Courrier envoy&AOk-");
        assert_eq!(decode("Courrier envoy&AOk-"), "Courrier envoyé");
        assert_eq!(encode("A&B"), "A&-B");
        assert_eq!(decode("A&-B"), "A&B");
        assert_eq!(decode(&encode("日本語")), "日本語");
    }
}
