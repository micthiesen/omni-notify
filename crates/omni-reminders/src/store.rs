//! Encrypted private state (`src/reminders/store.ts`).
//!
//! Separate from the docstore so generic data browsing/export cannot expose Apple
//! secrets. File format: `0x01 | iv(12) | tag(16) | AES-256-GCM ciphertext` with AAD
//! `omni-reminders:v1:<sha256(lowercase account)>`; the plaintext is the JSON state.
//! The directory is 0700, files 0600, opened with `O_NOFOLLOW`, at most 4 MiB, written
//! by atomic rename after fsync.

use std::fs;
use std::io::{Read as _, Write as _};
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use aes_gcm::aead::{Aead as _, KeyInit as _, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use futures::future::BoxFuture;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
const HEADER_BYTES: usize = 1 + 12 + 16;

/// Storage failed; never carries paths, keys or contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("Reminders private storage unavailable")]
pub struct RemindersStoreError {
    pub operation: StoreOperation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreOperation {
    Read,
    Write,
}

/// A mutation ledger entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OperationState {
    Reserved,
    Confirmed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredOperation {
    pub fingerprint: String,
    pub record_id: String,
    pub state: OperationState,
    /// `Schema.optional(Schema.Unknown)`: absent stays absent, a stored `null` stays
    /// `Some(Value::Null)` so a rewrite keeps the TypeScript document unchanged.
    #[serde(
        default,
        deserialize_with = "present_value",
        skip_serializing_if = "Option::is_none"
    )]
    pub result: Option<Value>,
}

fn present_value<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

/// The decrypted document; `session` is the Apple session (`Schema.Unknown`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StoredState {
    pub version: StateVersion,
    pub session: Value,
    pub notified: bool,
    pub operations: IndexMap<String, StoredOperation>,
}

/// The literal `1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StateVersion;

impl Serialize for StateVersion {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u8(1)
    }
}

impl<'de> Deserialize<'de> for StateVersion {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = f64::deserialize(deserializer)?;
        if value == 1.0 {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom("unsupported state version"))
        }
    }
}

impl Default for StoredState {
    fn default() -> Self {
        Self {
            version: StateVersion,
            session: Value::Null,
            notified: false,
            operations: IndexMap::new(),
        }
    }
}

/// `emptyRemindersState()`.
pub fn empty_reminders_state() -> StoredState {
    StoredState::default()
}

/// The private store seam (tests use in-memory fakes).
pub trait RemindersStore: Send + Sync {
    fn read(&self) -> BoxFuture<'_, Result<StoredState, RemindersStoreError>>;
    fn write(&self, state: StoredState) -> BoxFuture<'_, Result<(), RemindersStoreError>>;
}

/// The encrypted file store.
#[derive(Clone)]
pub struct FileRemindersStore {
    directory: PathBuf,
    filename: PathBuf,
    key: Option<[u8; 32]>,
    aad: Vec<u8>,
}

impl std::fmt::Debug for FileRemindersStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileRemindersStore")
            .field("directory", &self.directory)
            .finish_non_exhaustive()
    }
}

impl FileRemindersStore {
    /// `createRemindersStore(directory, keyHex, account)`; an invalid key fails
    /// every read and write rather than construction.
    pub fn new(directory: impl Into<PathBuf>, key_hex: &str, account: &str) -> Self {
        let directory = directory.into();
        let identity = omni_core::digest::sha256_hex(account.to_lowercase());
        let key = (crate::config::is_storage_key(key_hex))
            .then(|| hex::decode(key_hex).ok())
            .flatten()
            .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok());
        Self {
            filename: directory.join(format!("{identity}.enc")),
            directory,
            key,
            aad: format!("omni-reminders:v1:{identity}").into_bytes(),
        }
    }

    /// The state file path (tests).
    pub fn path(&self) -> &Path {
        &self.filename
    }

    fn prepare(&self) -> Option<Aes256Gcm> {
        let key = self.key?;
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        builder.create(&self.directory).ok()?;
        let meta = fs::symlink_metadata(&self.directory).ok()?;
        if !meta.is_dir() || meta.file_type().is_symlink() {
            return None;
        }
        fs::set_permissions(&self.directory, fs::Permissions::from_mode(0o700)).ok()?;
        Aes256Gcm::new_from_slice(&key).ok()
    }

    fn read_sync(&self) -> Option<StoredState> {
        let cipher = self.prepare()?;
        let mut file = match fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&self.filename)
        {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Some(empty_reminders_state());
            }
            Err(_) => return None,
        };
        let meta = file.metadata().ok()?;
        if !meta.is_file() || meta.len() > MAX_FILE_BYTES || meta.permissions().mode() & 0o077 != 0
        {
            return None;
        }
        let mut bytes = Vec::with_capacity(usize::try_from(meta.len()).ok()?);
        file.read_to_end(&mut bytes).ok()?;
        if bytes.len() < HEADER_BYTES || bytes[0] != 1 {
            return None;
        }
        let iv = &bytes[1..13];
        let tag = &bytes[13..29];
        let mut sealed = bytes[29..].to_vec();
        sealed.extend_from_slice(tag);
        let nonce = Nonce::try_from(iv).ok()?;
        let plain = cipher
            .decrypt(
                &nonce,
                Payload {
                    msg: &sealed,
                    aad: &self.aad,
                },
            )
            .ok()?;
        serde_json::from_slice(&plain).ok()
    }

    fn write_sync(&self, state: &StoredState) -> Option<()> {
        let cipher = self.prepare()?;
        let value = serde_json::to_value(state).ok()?;
        let plain = omni_core::js::json_stringify(&value).into_bytes();
        if plain.len() as u64 > MAX_FILE_BYTES {
            return None;
        }
        let iv: [u8; 12] = rand::random();
        let nonce = Nonce::try_from(&iv[..]).ok()?;
        let sealed = cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: &plain,
                    aad: &self.aad,
                },
            )
            .ok()?;
        let (ciphertext, tag) = sealed.split_at(sealed.len().checked_sub(16)?);
        let mut bytes = Vec::with_capacity(HEADER_BYTES + ciphertext.len());
        bytes.push(1);
        bytes.extend_from_slice(&iv);
        bytes.extend_from_slice(tag);
        bytes.extend_from_slice(ciphertext);
        let suffix: [u8; 12] = rand::random();
        let mut temporary = self.filename.clone().into_os_string();
        temporary.push(format!(".{}.tmp", hex::encode(suffix)));
        let temporary = PathBuf::from(temporary);
        let result = (|| {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&temporary)
                .ok()?;
            file.write_all(&bytes).ok()?;
            file.sync_all().ok()?;
            drop(file);
            fs::rename(&temporary, &self.filename).ok()?;
            fs::File::open(&self.directory).ok()?.sync_all().ok()
        })();
        let _ = fs::remove_file(&temporary);
        result
    }
}

impl RemindersStore for FileRemindersStore {
    fn read(&self) -> BoxFuture<'_, Result<StoredState, RemindersStoreError>> {
        let this = self.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || this.read_sync())
                .await
                .ok()
                .flatten()
                .ok_or(RemindersStoreError {
                    operation: StoreOperation::Read,
                })
        })
    }

    fn write(&self, state: StoredState) -> BoxFuture<'_, Result<(), RemindersStoreError>> {
        let this = self.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || this.write_sync(&state))
                .await
                .ok()
                .flatten()
                .ok_or(RemindersStoreError {
                    operation: StoreOperation::Write,
                })
        })
    }
}
