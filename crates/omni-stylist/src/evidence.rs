//! Private, content-addressed evidence bytes.
//!
//! Files live at `<private>/evidence/sha256/<aa>/<digest>` in directories with
//! mode 0700 and files with mode 0600. A write goes to a temporary file, is
//! synced and renamed into place, so a reader never sees a partial file. Every
//! read re-hashes the bytes and fails with `integrity` on a mismatch. Nothing
//! here is served over HTTP; bytes leave only through authenticated MCP tools.

use std::collections::HashSet;
use std::fs;
use std::io::Write as _;
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::api::EvidenceMime;
use crate::error::{StylistError, io, reject};

/// The decoded size limit: the MCP request limit is 4 MiB and base64 adds a third.
pub const MAX_EVIDENCE_BYTES: usize = 3_000_000;

/// SHA-256 of `bytes` as lowercase hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Checks that `bytes` really are `mime`.
pub fn validate_bytes(mime: EvidenceMime, bytes: &[u8]) -> Result<(), StylistError> {
    let unsupported = |what: &str| {
        reject(
            "unsupported_media",
            format!("the bytes are not {what}; declare the actual type"),
        )
    };
    if bytes.is_empty() {
        return Err(reject("unsupported_media", "evidence is empty"));
    }
    if bytes.len() > MAX_EVIDENCE_BYTES {
        return Err(reject(
            "too_large",
            format!(
                "evidence is {} bytes; the limit is {MAX_EVIDENCE_BYTES}. Resize photos to about 2048 px or split documents",
                bytes.len()
            ),
        ));
    }
    match mime {
        EvidenceMime::Jpeg => bytes
            .starts_with(&[0xFF, 0xD8, 0xFF])
            .then_some(())
            .ok_or_else(|| unsupported("a JPEG")),
        EvidenceMime::Png => bytes
            .starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A])
            .then_some(())
            .ok_or_else(|| unsupported("a PNG")),
        EvidenceMime::Webp => {
            (bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP")
                .then_some(())
                .ok_or_else(|| unsupported("a WebP image"))
        }
        EvidenceMime::Heic | EvidenceMime::Heif => {
            const BRANDS: [&[u8]; 9] = [
                b"heic", b"heix", b"hevc", b"hevx", b"heim", b"heis", b"mif1", b"msf1", b"heif",
            ];
            (bytes.len() >= 12 && &bytes[4..8] == b"ftyp" && BRANDS.contains(&&bytes[8..12]))
                .then_some(())
                .ok_or_else(|| unsupported("a HEIC/HEIF image"))
        }
        EvidenceMime::Pdf => bytes
            .starts_with(b"%PDF-")
            .then_some(())
            .ok_or_else(|| unsupported("a PDF")),
        EvidenceMime::Text | EvidenceMime::Markdown => {
            let text = std::str::from_utf8(bytes).map_err(|_| unsupported("UTF-8 text"))?;
            if text.contains('\0') {
                return Err(unsupported("text without NUL characters"));
            }
            Ok(())
        }
        EvidenceMime::Json => {
            let text = std::str::from_utf8(bytes).map_err(|_| unsupported("UTF-8 JSON"))?;
            serde_json::from_str::<serde_json::Value>(text)
                .map(|_| ())
                .map_err(|_| unsupported("valid JSON"))
        }
    }
}

/// The MIME type string of `mime`.
pub fn mime_str(mime: EvidenceMime) -> &'static str {
    match mime {
        EvidenceMime::Jpeg => "image/jpeg",
        EvidenceMime::Png => "image/png",
        EvidenceMime::Webp => "image/webp",
        EvidenceMime::Heic => "image/heic",
        EvidenceMime::Heif => "image/heif",
        EvidenceMime::Pdf => "application/pdf",
        EvidenceMime::Text => "text/plain",
        EvidenceMime::Markdown => "text/markdown",
        EvidenceMime::Json => "application/json",
    }
}

/// The private directory tree.
#[derive(Clone, Debug)]
pub struct PrivateDir {
    root: PathBuf,
}

fn private_dir(path: &Path) -> Result<(), StylistError> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .map_err(io)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(io)
}

impl PrivateDir {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn archives(&self) -> PathBuf {
        self.root.join("archives")
    }

    fn evidence_root(&self) -> PathBuf {
        self.root.join("evidence").join("sha256")
    }

    /// Creates the tree with mode 0700.
    pub fn ensure(&self) -> Result<(), StylistError> {
        for dir in [self.root.clone(), self.evidence_root(), self.archives()] {
            private_dir(&dir)?;
        }
        Ok(())
    }

    pub fn evidence_path(&self, sha256: &str) -> PathBuf {
        let shard = sha256.get(0..2).unwrap_or("00");
        self.evidence_root().join(shard).join(sha256)
    }

    /// Writes `bytes` under their digest unless an intact copy exists.
    pub fn put(&self, bytes: &[u8]) -> Result<Stored, StylistError> {
        let digest = sha256_hex(bytes);
        let path = self.evidence_path(&digest);
        if let Ok(existing) = fs::read(&path)
            && sha256_hex(&existing) == digest
        {
            return Ok(Stored {
                digest,
                created: false,
            });
        }
        if let Some(parent) = path.parent() {
            private_dir(parent)?;
        }
        self.write_atomic(&path, bytes)?;
        Ok(Stored {
            digest,
            created: true,
        })
    }

    /// Writes `bytes` to `path` (mode 0600) through a synced temporary file in
    /// the same directory, so the rename never crosses filesystems.
    pub fn write_atomic(&self, path: &Path, bytes: &[u8]) -> Result<(), StylistError> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let temp = parent.join(format!(".{}.part", uuid::Uuid::new_v4().simple()));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)
            .map_err(io)?;
        let written = file.write_all(bytes).and_then(|()| file.sync_all());
        drop(file);
        if let Err(error) = written {
            let _ = fs::remove_file(&temp);
            return Err(io(error));
        }
        if let Err(error) = fs::rename(&temp, path) {
            let _ = fs::remove_file(&temp);
            return Err(io(error));
        }
        if let Ok(dir) = fs::File::open(parent) {
            let _ = dir.sync_all();
        }
        Ok(())
    }

    /// The bytes for `sha256`, verified.
    pub fn get(&self, sha256: &str) -> Result<Vec<u8>, StylistError> {
        if !is_digest(sha256) {
            return Err(reject(
                "integrity",
                "an evidence digest is not 64 hex characters",
            ));
        }
        let path = self.evidence_path(sha256);
        let bytes = fs::read(&path).map_err(|e| {
            reject(
                "integrity",
                format!("evidence file {sha256} is unreadable: {e}"),
            )
        })?;
        let actual = sha256_hex(&bytes);
        if actual != sha256 {
            return Err(reject(
                "integrity",
                format!("evidence file {sha256} hashes to {actual}; it is corrupt"),
            ));
        }
        Ok(bytes)
    }

    /// Removes the files for `candidates` that are not in `keep`; returns the
    /// count. Only named digests are ever removed, never a sweep of the
    /// directory, so a database that lost its rows cannot empty it.
    pub fn remove_digests(
        &self,
        candidates: &HashSet<String>,
        keep: &HashSet<String>,
    ) -> Result<u64, StylistError> {
        let mut removed = 0;
        for digest in candidates
            .iter()
            .filter(|d| is_digest(d) && !keep.contains(*d))
        {
            match fs::remove_file(self.evidence_path(digest)) {
                Ok(()) => removed += 1,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(io(error)),
            }
        }
        Ok(removed)
    }

    /// Removes temporary `.part` files older than an hour (left by a crash).
    pub fn remove_stale_parts(&self) -> u64 {
        let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        let mut dirs = vec![self.archives()];
        if let Ok(shards) = fs::read_dir(self.evidence_root()) {
            dirs.extend(shards.flatten().map(|s| s.path()));
        }
        let mut removed = 0;
        for dir in dirs {
            let Ok(files) = fs::read_dir(dir) else {
                continue;
            };
            for file in files.flatten() {
                let stale = file
                    .metadata()
                    .and_then(|m| m.modified())
                    .is_ok_and(|modified| modified < cutoff);
                let name = file.file_name().to_string_lossy().into_owned();
                if name.starts_with('.')
                    && name.ends_with(".part")
                    && stale
                    && fs::remove_file(file.path()).is_ok()
                {
                    removed += 1;
                }
            }
        }
        removed
    }
}

/// A written evidence file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stored {
    pub digest: String,
    /// This call created the file (it did not exist intact before).
    pub created: bool,
}

/// Whether `value` is a lowercase hex SHA-256 digest.
pub fn is_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_must_match_the_declared_type() {
        let jpeg = [0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10];
        assert!(validate_bytes(EvidenceMime::Jpeg, &jpeg).is_ok());
        assert!(validate_bytes(EvidenceMime::Png, &jpeg).is_err());
        let mut heic = vec![0, 0, 0, 0x18];
        heic.extend_from_slice(b"ftypheic");
        assert!(validate_bytes(EvidenceMime::Heic, &heic).is_ok());
        assert!(validate_bytes(EvidenceMime::Pdf, b"%PDF-1.7\n").is_ok());
        assert!(validate_bytes(EvidenceMime::Json, b"{\"a\":1}").is_ok());
        assert!(validate_bytes(EvidenceMime::Json, b"{oops").is_err());
        assert!(validate_bytes(EvidenceMime::Text, b"a\0b").is_err());
        let big = vec![b'a'; MAX_EVIDENCE_BYTES + 1];
        assert!(matches!(
            validate_bytes(EvidenceMime::Text, &big),
            Err(StylistError::Rejected {
                code: "too_large",
                ..
            })
        ));
    }

    #[test]
    fn files_are_private_and_verified() {
        let dir = tempfile::tempdir().unwrap();
        let private = PrivateDir::new(dir.path().join("stylist-private"));
        private.ensure().unwrap();
        let stored = private.put(b"photo bytes").unwrap();
        assert!(stored.created);
        let digest = stored.digest;
        assert_eq!(private.get(&digest).unwrap(), b"photo bytes");
        assert!(!private.put(b"photo bytes").unwrap().created);
        assert!(private.get("../../etc/passwd").is_err());
        let path = private.evidence_path(&digest);
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let root_mode = fs::metadata(private.root()).unwrap().permissions().mode() & 0o777;
        assert_eq!(root_mode, 0o700);
        fs::write(&path, b"tampered").unwrap();
        assert!(matches!(
            private.get(&digest),
            Err(StylistError::Rejected {
                code: "integrity",
                ..
            })
        ));
        let repaired = private.put(b"photo bytes").unwrap();
        assert_eq!(repaired.digest, digest);
        assert!(repaired.created, "a corrupt copy is rewritten");
        assert_eq!(private.get(&digest).unwrap(), b"photo bytes");
        let other = private.put(b"other photo").unwrap().digest;
        let candidates: HashSet<String> = [digest.clone(), other.clone()].into();
        let keep: HashSet<String> = [other.clone()].into();
        assert_eq!(private.remove_digests(&candidates, &keep).unwrap(), 1);
        assert!(private.get(&digest).is_err());
        assert_eq!(private.get(&other).unwrap(), b"other photo");
        assert_eq!(private.remove_digests(&candidates, &keep).unwrap(), 0);
    }
}
