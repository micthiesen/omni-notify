//! Cheap structural checks of the speech model files.
//!
//! sherpa-onnx aborts the whole process (an uncaught C++ exception) when it
//! is handed an empty, truncated or non-ONNX file, so every file is checked
//! here first and a bad one becomes an ordinary error.
//!
//! An `.onnx` file is a protobuf `ModelProto`. The check walks its top-level
//! fields without reading the payloads: every field must have a valid wire
//! type and end inside the file, and `ir_version` (1) and `graph` (7) must be
//! present. A token table must be UTF-8 lines whose last field is an integer
//! id, as sherpa-onnx's `SymbolTable` reads them.

use std::fs::File;
use std::io::{BufReader, ErrorKind, Read, Seek};
use std::path::Path;

const IR_VERSION_FIELD: u64 = 1;
const GRAPH_FIELD: u64 = 7;
/// The token table is read whole; Parakeet's is about 94 KB.
const MAX_TOKENS_BYTES: u64 = 16 * 1024 * 1024;

/// Validates `path` by its extension: `.txt` as a token table, anything
/// else as an ONNX model.
pub fn check_model_file(path: &Path) -> Result<(), String> {
    let is_tokens = path.extension().is_some_and(|ext| ext == "txt");
    if is_tokens {
        check_tokens(path)
    } else {
        check_onnx(path)
    }
}

fn file_size(path: &Path) -> Result<(File, u64), String> {
    let file = File::open(path).map_err(|e| format!("cannot open: {e}"))?;
    let size = file
        .metadata()
        .map_err(|e| format!("cannot stat: {e}"))?
        .len();
    if size == 0 {
        return Err("file is empty".to_owned());
    }
    Ok((file, size))
}

/// One base-128 varint, or `None` at a clean end of file.
fn read_varint(reader: &mut impl Read, at_field_start: bool) -> Result<Option<u64>, String> {
    let mut value: u64 = 0;
    for index in 0..10 {
        let mut byte = [0u8; 1];
        match reader.read_exact(&mut byte) {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::UnexpectedEof => {
                return if index == 0 && at_field_start {
                    Ok(None)
                } else {
                    Err("truncated varint".to_owned())
                };
            }
            Err(e) => return Err(format!("read failed: {e}")),
        }
        value |= u64::from(byte[0] & 0x7f) << (7 * index);
        if byte[0] & 0x80 == 0 {
            return Ok(Some(value));
        }
    }
    Err("malformed varint".to_owned())
}

fn check_onnx(path: &Path) -> Result<(), String> {
    let (file, size) = file_size(path)?;
    let mut reader = BufReader::new(file);
    let mut position: u64 = 0;
    let (mut has_ir_version, mut has_graph) = (false, false);
    let not_onnx = |detail: String| format!("not a valid ONNX model ({detail})");
    while let Some(tag) = read_varint(&mut reader, true).map_err(not_onnx)? {
        let field = tag >> 3;
        if field == 0 {
            return Err(not_onnx(format!("field 0 at byte {position}")));
        }
        let skip = match tag & 7 {
            0 => {
                read_varint(&mut reader, false).map_err(not_onnx)?;
                0
            }
            1 => 8,
            2 => read_varint(&mut reader, false)
                .map_err(not_onnx)?
                .unwrap_or_default(),
            5 => 4,
            wire => {
                return Err(not_onnx(format!("wire type {wire} at byte {position}")));
            }
        };
        let here = reader
            .stream_position()
            .map_err(|e| format!("seek failed: {e}"))?;
        let end = here
            .checked_add(skip)
            .filter(|end| *end <= size)
            .ok_or_else(|| {
                format!("truncated: field {field} needs {skip} bytes at byte {here} of {size}")
            })?;
        let offset = i64::try_from(skip).map_err(|_| not_onnx("oversized field".to_owned()))?;
        reader
            .seek_relative(offset)
            .map_err(|e| format!("seek failed: {e}"))?;
        position = end;
        has_ir_version |= field == IR_VERSION_FIELD;
        has_graph |= field == GRAPH_FIELD;
    }
    match (has_ir_version, has_graph) {
        (true, true) => Ok(()),
        (false, _) => Err(not_onnx("no ir_version".to_owned())),
        (true, false) => Err(not_onnx("no graph".to_owned())),
    }
}

fn check_tokens(path: &Path) -> Result<(), String> {
    let (file, size) = file_size(path)?;
    if size > MAX_TOKENS_BYTES {
        return Err(format!("token table is {size} bytes, over the limit"));
    }
    let mut text = String::new();
    BufReader::new(file)
        .read_to_string(&mut text)
        .map_err(|e| format!("not a UTF-8 token table: {e}"))?;
    let mut tokens = 0usize;
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let id = line.split_whitespace().last().unwrap_or_default();
        if id.parse::<i64>().is_err() {
            return Err(format!(
                "not a token table: line {} has no integer id",
                index + 1
            ));
        }
        tokens += 1;
    }
    if tokens == 0 {
        return Err("token table has no tokens".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn write(dir: &Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    /// `ir_version: 8`, `producer_name: "t"`, `graph: <3 bytes>`.
    const MINIMAL_ONNX: &[u8] = &[0x08, 0x08, 0x12, 0x01, b't', 0x3a, 0x03, 1, 2, 3];

    #[test]
    fn accepts_a_well_formed_model_and_token_table() {
        let dir = tempfile::tempdir().unwrap();
        let model = write(dir.path(), "m.onnx", MINIMAL_ONNX);
        assert_eq!(check_model_file(&model), Ok(()));
        let tokens = write(dir.path(), "tokens.txt", "<unk> 0\n▁the 1\n 2\n".as_bytes());
        assert_eq!(check_model_file(&tokens), Ok(()));
    }

    #[test]
    fn rejects_empty_truncated_and_foreign_models() {
        let dir = tempfile::tempdir().unwrap();
        let empty = write(dir.path(), "empty.onnx", b"");
        assert_eq!(check_model_file(&empty).unwrap_err(), "file is empty");
        let truncated = write(
            dir.path(),
            "cut.onnx",
            &MINIMAL_ONNX[..MINIMAL_ONNX.len() - 1],
        );
        assert!(
            check_model_file(&truncated)
                .unwrap_err()
                .starts_with("truncated: field 7")
        );
        let html = write(dir.path(), "page.onnx", b"<html>Not Found</html>");
        assert!(
            check_model_file(&html)
                .unwrap_err()
                .starts_with("not a valid ONNX model")
        );
        let graphless = write(dir.path(), "nograph.onnx", &[0x08, 0x08]);
        assert_eq!(
            check_model_file(&graphless).unwrap_err(),
            "not a valid ONNX model (no graph)"
        );
        let missing = dir.path().join("absent.onnx");
        assert!(
            check_model_file(&missing)
                .unwrap_err()
                .starts_with("cannot open")
        );
    }

    #[test]
    fn rejects_bad_token_tables() {
        let dir = tempfile::tempdir().unwrap();
        let empty = write(dir.path(), "tokens.txt", b"");
        assert_eq!(check_model_file(&empty).unwrap_err(), "file is empty");
        let blank = write(dir.path(), "blank.txt", b"\n\n");
        assert_eq!(
            check_model_file(&blank).unwrap_err(),
            "token table has no tokens"
        );
        let words = write(dir.path(), "words.txt", b"<unk> 0\nhello world\n");
        assert_eq!(
            check_model_file(&words).unwrap_err(),
            "not a token table: line 2 has no integer id"
        );
        let binary = write(dir.path(), "bin.txt", &[0xff, 0xfe, 0x00]);
        assert!(
            check_model_file(&binary)
                .unwrap_err()
                .starts_with("not a UTF-8")
        );
    }
}
