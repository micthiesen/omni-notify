//! Apple's zlib-compressed versioned topotext documents (`src/reminders/codec.ts`).
//!
//! Adapted from the MIT-licensed iobroker.icloud reminders implementation
//! (07a91933e3f05a36d9c8918ece7f3de295aef805); see docs/server-reminders.md.

use std::io::{Read as _, Write as _};

use flate2::Compression;
use flate2::bufread::GzDecoder;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;

use crate::json::{base64_encode, node_base64};

const MAX_TEXT_BYTES: usize = 64 * 1024;
const REPLICA_UUID: [u8; 16] = [
    0xd4, 0x6b, 0xca, 0xe4, 0x1b, 0x87, 0x66, 0xc1, 0x8d, 0x75, 0xef, 0xe3, 0x5c, 0x91, 0x45, 0xc3,
];

/// A document that cannot be encoded or decoded; the text is never included.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CodecError {
    #[error("Reminder text exceeds the size limit")]
    TooLarge,
    #[error("Invalid reminder document encoding")]
    Encoding,
    #[error("Invalid reminder document size")]
    Size,
    #[error("Cannot decode reminder document")]
    Document,
}

fn varint(out: &mut Vec<u8>, value: u64) {
    let mut n = value & 0xffff_ffff;
    loop {
        let more = n > 0x7f;
        out.push(((n & 0x7f) as u8) | if more { 0x80 } else { 0 });
        n >>= 7;
        if !more {
            break;
        }
    }
}

fn field(number: u64, value: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(value.len() + 4);
    varint(&mut out, (number << 3) | 2);
    varint(&mut out, value.len() as u64);
    out.extend_from_slice(value);
    out
}

fn integer(number: u64, value: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(6);
    varint(&mut out, number << 3);
    varint(&mut out, value);
    out
}

fn char_id(replica: u64, clock: u64) -> Vec<u8> {
    [integer(1, replica), integer(2, clock)].concat()
}

/// The uncompressed protobuf for `text` (exposed for golden tests).
pub fn crdt_protobuf(text: &str) -> Result<Vec<u8>, CodecError> {
    if text.len() > MAX_TEXT_BYTES {
        return Err(CodecError::TooLarge);
    }
    // JS `text.length`: UTF-16 code units.
    let length = omni_core::js::utf16_len(text) as u64;
    let sentinel = [
        field(1, &char_id(0, 0)),
        integer(2, 0),
        field(3, &char_id(0, 0)),
        integer(5, 1),
    ]
    .concat();
    let content = [
        field(1, &char_id(1, 0)),
        integer(2, length),
        field(3, &char_id(1, 0)),
        integer(5, 2),
    ]
    .concat();
    let terminal = [
        field(1, &char_id(0, 0xffff_ffff)),
        integer(2, 0),
        field(3, &char_id(0, 0xffff_ffff)),
    ]
    .concat();
    let clock = field(
        1,
        &[
            field(1, &REPLICA_UUID),
            field(2, &integer(1, length)),
            field(2, &integer(1, 1)),
        ]
        .concat(),
    );
    let mut string = field(2, text.as_bytes());
    string.extend(field(3, &sentinel));
    if length > 0 {
        string.extend(field(3, &content));
    }
    string.extend(field(3, &terminal));
    string.extend(field(4, &clock));
    if length > 0 {
        string.extend(field(5, &integer(1, length)));
    }
    let version = [integer(1, 0), integer(2, 0), field(3, &string)].concat();
    Ok([integer(1, 0), field(2, &version)].concat())
}

/// Apple stores reminder text in a zlib-compressed versioned topotext document.
pub fn encode_crdt_document(text: &str) -> Result<String, CodecError> {
    let proto = crdt_protobuf(text)?;
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(&proto)
        .and_then(|()| encoder.finish())
        .map(|compressed| base64_encode(&compressed))
        .map_err(|_| CodecError::Encoding)
}

fn read_varint(data: &[u8], start: usize) -> Option<(u64, usize)> {
    let mut value: u64 = 0;
    let mut offset = start;
    let mut shift = 0u32;
    while shift <= 28 && offset < data.len() {
        let byte = data[offset];
        offset += 1;
        value += u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some((value, offset));
        }
        shift += 7;
    }
    None
}

/// Every length-delimited field `target` of a message; `None` when malformed.
fn bytes_at(data: &[u8], target: u64) -> Option<Vec<&[u8]>> {
    let mut result = Vec::new();
    let mut offset = 0usize;
    while offset < data.len() {
        let (tag, next) = read_varint(data, offset)?;
        offset = next;
        let number = tag / 8;
        let wire = tag & 7;
        if number == 0 {
            return None;
        }
        match wire {
            2 => {
                let (length, next) = read_varint(data, offset)?;
                let length = usize::try_from(length).ok()?;
                if length > data.len() - next {
                    return None;
                }
                offset = next;
                if number == target {
                    result.push(&data[offset..offset + length]);
                }
                offset += length;
            }
            0 => {
                let (_, next) = read_varint(data, offset)?;
                offset = next;
            }
            1 | 5 => {
                offset += if wire == 1 { 8 } else { 4 };
                if offset > data.len() {
                    return None;
                }
            }
            _ => return None,
        }
    }
    Some(result)
}

/// `Buffer#toString("utf8")`: invalid sequences become U+FFFD.
fn utf8(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn document_text(data: &[u8]) -> Option<String> {
    let versions = bytes_at(data, 2)?;
    for version in versions {
        let Some(strings) = bytes_at(version, 3) else {
            continue;
        };
        for string in strings {
            if let Some(values) = bytes_at(string, 2)
                && let Some(first) = values.first()
            {
                return Some(utf8(first));
            }
        }
    }
    // CloudKit has also returned bare Version and topotext.String documents.
    for string in bytes_at(data, 3).unwrap_or_default() {
        if let Some(values) = bytes_at(string, 2)
            && let Some(first) = values.first()
        {
            return Some(utf8(first));
        }
    }
    None
}

fn is_base64_text(value: &str) -> bool {
    let trimmed = value.trim_end_matches('=');
    value.len() - trimmed.len() <= 2
        && trimmed
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
}

const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];

/// `unzipSync` with `maxOutputLength`: gzip or zlib, auto-detected. Like Node, gzip
/// input may hold several members and bytes after the last member are ignored, as
/// are bytes after a zlib stream; truncated or corrupt streams fail.
fn unzip(compressed: &[u8], max: usize) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let limit = (max as u64) + 1;
    if compressed.starts_with(&GZIP_MAGIC) {
        let mut rest = compressed;
        while rest.starts_with(&GZIP_MAGIC) {
            let budget = limit.saturating_sub(out.len() as u64);
            let mut decoder = GzDecoder::new(&mut rest);
            decoder.by_ref().take(budget).read_to_end(&mut out).ok()?;
            if out.len() > max {
                return None;
            }
            drop(decoder);
        }
    } else {
        ZlibDecoder::new(compressed)
            .take(limit)
            .read_to_end(&mut out)
            .ok()?;
    }
    (out.len() <= max).then_some(out)
}

/// Decodes a stored document exactly; ciphertext never decodes as text.
pub fn decode_crdt_document(value: &str) -> Result<String, CodecError> {
    if value.len() > MAX_TEXT_BYTES * 2 || !is_base64_text(value) {
        return Err(CodecError::Encoding);
    }
    let compressed = node_base64(value);
    if compressed.is_empty() || compressed.len() > MAX_TEXT_BYTES {
        return Err(CodecError::Size);
    }
    // CloudKit also returns uncompressed protobuf documents. The same strict
    // parser must accept the document; ciphertext is never shown as text.
    let data = unzip(&compressed, MAX_TEXT_BYTES * 2).unwrap_or(compressed);
    // Preserve the stored text exactly so write verification cannot change meaning.
    document_text(&data).ok_or(CodecError::Document)
}

#[cfg(test)]
mod codec_spec {
    //! Port of `src/reminders/codec.spec.ts`.
    use super::*;
    use flate2::write::GzEncoder;

    fn inflate(encoded: &str) -> Vec<u8> {
        let mut out = Vec::new();
        ZlibDecoder::new(node_base64(encoded).as_slice())
            .read_to_end(&mut out)
            .unwrap();
        out
    }

    #[test]
    fn decodes_server_decrypted_gzip_and_raw_documents() {
        let text = "ADP reminder 🌿";
        let proto = inflate(&encode_crdt_document(text).unwrap());
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        gz.write_all(&proto).unwrap();
        let gzip = gz.finish().unwrap();
        for wire in [gzip, proto] {
            assert_eq!(decode_crdt_document(&base64_encode(&wire)).unwrap(), text);
        }
    }

    /// Rust-only: Node's `unzipSync` joins gzip members, ignores trailing bytes, and
    /// rejects truncated or corrupt streams (falling back to the strict raw parser).
    #[test]
    fn unzips_like_node_for_members_trailing_bytes_and_truncation() {
        let text = "Split 🌿";
        let proto = inflate(&encode_crdt_document(text).unwrap());
        let (head, tail) = proto.split_at(proto.len() / 2);
        let gzip = |bytes: &[u8]| {
            let mut gz = GzEncoder::new(Vec::new(), Compression::default());
            gz.write_all(bytes).unwrap();
            gz.finish().unwrap()
        };
        let members = [gzip(head), gzip(tail), vec![0, 0, 7]].concat();
        assert_eq!(
            decode_crdt_document(&base64_encode(&members)).unwrap(),
            text
        );
        let zlib = node_base64(&encode_crdt_document(text).unwrap());
        let trailing = [zlib.clone(), vec![1, 2, 3]].concat();
        assert_eq!(
            decode_crdt_document(&base64_encode(&trailing)).unwrap(),
            text
        );
        assert_eq!(unzip(&zlib[..zlib.len() - 3], MAX_TEXT_BYTES * 2), None);
        let mut corrupt = zlib.clone();
        let last = corrupt.len() - 1;
        corrupt[last] ^= 1;
        assert_eq!(unzip(&corrupt, MAX_TEXT_BYTES * 2), None);
    }

    #[test]
    fn rejects_ciphertext_and_oversized_decompression() {
        assert!(decode_crdt_document(&base64_encode(&[3u8; 64])).is_err());
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        gz.write_all(&vec![65u8; 256 * 1024]).unwrap();
        assert!(decode_crdt_document(&base64_encode(&gz.finish().unwrap())).is_err());
    }

    #[test]
    fn preserves_separators_and_controls_through_encoding_and_readback() {
        let value = "Title\u{2028}second line\u{2029}paragraph\u{1}\t🙂";
        assert_eq!(
            decode_crdt_document(&encode_crdt_document(value).unwrap()).unwrap(),
            value
        );
    }

    #[test]
    fn round_trips_unicode_and_rejects_malformed_documents() {
        assert_eq!(
            decode_crdt_document(&encode_crdt_document("Café 🌿\nMilk").unwrap()).unwrap(),
            "Café 🌿\nMilk"
        );
        assert!(decode_crdt_document("%%%").is_err());
    }
}
