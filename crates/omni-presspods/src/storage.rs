//! Episode audio files and per-chunk synthesis checkpoints.
//!
//! Final MP3s live at `<audioDir>/<episodeId>.mp3`. Each verified chunk's
//! prepared WAV is cached at `<audioDir>/.chunks/<workId>/<key>.wav`, keyed
//! by article identity, render signature and chunk text, so a process killed
//! mid-synthesis resumes from the last good chunk. The cache is strictly an
//! optimization: every checkpoint read and write is best-effort.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use sha2::{Digest as _, Sha256};

use crate::error::PressPodsError;

const LOG: &str = "PressPods";

static AUDIO_FILE: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_-]+\.mp3$").ok());

/// Only content-addressed names we generated ourselves are ever served.
pub fn is_audio_file_name(name: &str) -> bool {
    AUDIO_FILE.as_ref().is_some_and(|re| re.is_match(name))
}

/// `getAudioDir()`: `PRESSPODS_AUDIO_DIR`, else `press-pods-audio` next to the
/// database (mirroring the docstore path resolution, including Docker's
/// `/data/` prefix).
pub fn resolve_audio_dir(config: &omni_config::Config) -> PathBuf {
    if let Some(dir) = config
        .presspods_audio_dir
        .as_deref()
        .filter(|d| !d.is_empty())
    {
        return PathBuf::from(dir);
    }
    let db = config.db_path();
    db.parent()
        .unwrap_or_else(|| Path::new(""))
        .join("press-pods-audio")
}

/// Stable, filesystem-safe id for an article's checkpoint set.
pub fn checkpoint_work_id(normalized_url: &str) -> String {
    let digest = hex::encode(Sha256::digest(normalized_url.as_bytes()));
    digest[..16].to_owned()
}

/// Content-addressed checkpoint name: `sha256(signature \0 text).wav`. The NUL
/// keeps the signature/text boundary unambiguous.
pub fn checkpoint_key(signature: &str, text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(signature.as_bytes());
    hasher.update([0u8]);
    hasher.update(text.as_bytes());
    format!("{}.wav", hex::encode(hasher.finalize()))
}

/// The audio directory and its checkpoint cache.
#[derive(Clone, Debug)]
pub struct AudioStore {
    dir: PathBuf,
}

impl AudioStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub async fn ensure_dir(&self) -> Result<(), PressPodsError> {
        tokio::fs::create_dir_all(&self.dir).await.map_err(|e| {
            PressPodsError::io(format!("prepare audio directory {}", self.dir.display()), e)
        })
    }

    /// Path of a served episode file; rejects names we would never generate.
    pub fn episode_audio_path(&self, file_name: &str) -> Result<PathBuf, PressPodsError> {
        if !is_audio_file_name(file_name) {
            return Err(PressPodsError::invalid(
                "resolve episode audio path",
                format!("Invalid episode audio file name: {file_name}"),
            ));
        }
        Ok(self.dir.join(file_name))
    }

    pub async fn save_episode_audio(
        &self,
        file_name: &str,
        audio: &[u8],
    ) -> Result<(), PressPodsError> {
        self.ensure_dir().await?;
        let path = self.episode_audio_path(file_name)?;
        tokio::fs::write(&path, audio)
            .await
            .map_err(|e| PressPodsError::io("write episode audio", e))
    }

    /// Best-effort delete; never fails. The row is the source of truth and a
    /// leftover, unreferenced, unguessably named MP3 is harmless.
    pub async fn delete_episode_audio(&self, file_name: &str) {
        let Ok(path) = self.episode_audio_path(file_name) else {
            return;
        };
        match tokio::fs::remove_file(&path).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                tracing::debug!(target: LOG, file = file_name, error = %e, "Could not delete episode audio")
            }
        }
    }

    fn checkpoint_dir(&self, work_id: &str) -> PathBuf {
        self.dir.join(".chunks").join(work_id)
    }

    /// Cached prepared WAV bytes, or `None` on a miss or any read error.
    pub async fn read_chunk_checkpoint(&self, work_id: &str, key: &str) -> Option<Vec<u8>> {
        tokio::fs::read(self.checkpoint_dir(work_id).join(key))
            .await
            .ok()
    }

    /// Atomically caches a prepared chunk WAV (temp file + rename, so a kill
    /// mid-write never leaves a truncated file that reads as a valid take).
    /// Best-effort.
    pub async fn write_chunk_checkpoint(&self, work_id: &str, key: &str, wav: &[u8]) {
        let dir = self.checkpoint_dir(work_id);
        let tmp = dir.join(format!(".tmp-{}", random_hex(8)));
        let result = async {
            tokio::fs::create_dir_all(&dir).await?;
            tokio::fs::write(&tmp, wav).await?;
            tokio::fs::rename(&tmp, dir.join(key)).await
        }
        .await;
        if let Err(e) = result {
            tracing::debug!(target: LOG, error = %e, "Could not write chunk checkpoint");
            let _ignored = tokio::fs::remove_file(&tmp).await;
        }
    }

    /// Drops a single (for example corrupt) checkpoint so it is not retried forever.
    pub async fn delete_chunk_checkpoint(&self, work_id: &str, key: &str) {
        let _ignored = tokio::fs::remove_file(self.checkpoint_dir(work_id).join(key)).await;
    }

    /// Drops an article's whole checkpoint set (episode finished or abandoned).
    pub async fn clear_chunk_checkpoints(&self, work_id: &str) {
        match tokio::fs::remove_dir_all(self.checkpoint_dir(work_id)).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::debug!(target: LOG, error = %e, "Could not clear chunk checkpoints"),
        }
    }
}

/// `n` random bytes as lowercase hex (temporary file names).
pub(crate) fn random_hex(n: usize) -> String {
    let bytes: Vec<u8> = (0..n).map(|_| rand::random::<u8>()).collect();
    hex::encode(bytes)
}

#[cfg(test)]
mod storage_spec {
    //! Ports `src/press-pods/storage.spec.ts`. `materializeCheckpointWav` has
    //! no Rust counterpart (resumed checkpoints are materialized as owned
    //! temporary files by the audio chain, see `speech::audio_chain::TempFile`),
    //! so its case checks that path instead.
    use super::*;

    fn store() -> (AudioStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        (AudioStore::new(dir.path().join("press-pods-audio")), dir)
    }

    #[test]
    fn is_deterministic_and_filesystem_safe() {
        let id = checkpoint_work_id("https://example.com/a");
        assert_eq!(id, checkpoint_work_id("https://example.com/a"));
        assert_eq!(id.len(), 16);
        assert!(
            id.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    #[test]
    fn differs_for_different_urls() {
        assert_ne!(
            checkpoint_work_id("https://example.com/a"),
            checkpoint_work_id("https://example.com/b")
        );
    }

    #[test]
    fn is_deterministic_for_the_same_signature_text() {
        assert_eq!(
            checkpoint_key("sig", "hello world"),
            checkpoint_key("sig", "hello world")
        );
    }

    #[test]
    fn changes_with_the_render_signature() {
        assert_ne!(
            checkpoint_key("sigA", "text"),
            checkpoint_key("sigB", "text")
        );
    }

    #[test]
    fn has_an_unambiguous_signature_text_boundary() {
        assert_ne!(checkpoint_key("sig", "text"), checkpoint_key("si", "gtext"));
    }

    #[test]
    fn matches_the_node_digest_layout() {
        // sha256("sig\0text") computed by node's createHash.
        assert_eq!(
            checkpoint_key("sig", "text"),
            format!("{}.wav", hex::encode(Sha256::digest(b"sig\0text")))
        );
    }

    #[tokio::test]
    async fn returns_null_on_a_miss() {
        let (store, _dir) = store();
        assert!(
            store
                .read_chunk_checkpoint("workid-roundtrip", &checkpoint_key("s", "missing"))
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn writes_then_reads_back_the_exact_bytes() {
        let (store, _dir) = store();
        let key = checkpoint_key("s", "chunk one");
        store
            .write_chunk_checkpoint("w", &key, b"fake-wav-bytes")
            .await;
        assert_eq!(
            store.read_chunk_checkpoint("w", &key).await.as_deref(),
            Some(&b"fake-wav-bytes"[..])
        );
    }

    #[tokio::test]
    async fn deletes_a_single_checkpoint() {
        let (store, _dir) = store();
        let key = checkpoint_key("s", "chunk two");
        store.write_chunk_checkpoint("w", &key, b"x").await;
        store.delete_chunk_checkpoint("w", &key).await;
        assert!(store.read_chunk_checkpoint("w", &key).await.is_none());
    }

    #[tokio::test]
    async fn clears_the_whole_work_set() {
        let (store, _dir) = store();
        let key = checkpoint_key("s", "chunk three");
        store.write_chunk_checkpoint("w", &key, b"y").await;
        store.clear_chunk_checkpoints("w").await;
        assert!(store.read_chunk_checkpoint("w", &key).await.is_none());
    }

    #[tokio::test]
    async fn clearing_a_non_existent_work_set_is_a_no_op() {
        let (store, _dir) = store();
        store.clear_chunk_checkpoints("never-created").await;
    }

    #[tokio::test]
    async fn writes_the_bytes_to_a_fresh_readable_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = crate::speech::audio_chain::TempFile::write(dir.path(), "wav", b"materialized")
            .await
            .unwrap();
        assert_eq!(std::fs::read(file.path()).unwrap(), b"materialized");
        let path = file.path().to_owned();
        drop(file);
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn never_throws_for_a_missing_file() {
        let (store, _dir) = store();
        store.delete_episode_audio("does-not-exist.mp3").await;
    }

    #[tokio::test]
    async fn ignores_names_that_arent_valid_audio_files() {
        let (store, _dir) = store();
        store.delete_episode_audio("../escape").await;
        assert!(store.episode_audio_path("../escape").is_err());
    }
}
