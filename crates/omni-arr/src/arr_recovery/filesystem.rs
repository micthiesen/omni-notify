//! Deletion verification on the optional read-only download mounts.
//! Unlike Arr's filesystem API, a direct
//! directory read distinguishes an empty parent from an inaccessible one.

use std::future::Future;
use std::io;

use super::types::{ArrRecoveryError, ArrResult};
use crate::paths;

/// The download roots mounted read-only into the container.
pub const DOWNLOAD_ROOTS: [&str; 2] = ["/media/storage/nzbget/completed", "/tmp/inter"];

const OPERATION: &str = "verify download deletion";

/// Lists the entry names of a directory.
pub trait DirLister: Send + Sync {
    fn list(&self, dir: &str) -> impl Future<Output = io::Result<Vec<String>>> + Send;
}

/// Reads the real filesystem.
#[derive(Clone, Copy, Debug, Default)]
pub struct LocalDirs;

impl DirLister for LocalDirs {
    async fn list(&self, dir: &str) -> io::Result<Vec<String>> {
        let mut entries = tokio::fs::read_dir(dir).await?;
        let mut names = Vec::new();
        while let Some(entry) = entries.next_entry().await? {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
        Ok(names)
    }
}

/// `verifyDownloadRemoved`: `true` when the parent directory was read and no
/// longer contains the download; an error when the path is outside the
/// mounts or the parent cannot be read.
pub async fn verify_download_removed(output_path: &str, dirs: &impl DirLister) -> ArrResult<bool> {
    let normalized = paths::normalize(output_path);
    if !DOWNLOAD_ROOTS
        .iter()
        .any(|root| normalized.starts_with(&format!("{root}/")))
    {
        return Err(ArrRecoveryError::message(
            OPERATION,
            "Path is outside the read-only download mounts",
        ));
    }
    let entries = dirs.list(&paths::dirname(&normalized)).await.map_err(|_| {
        ArrRecoveryError::message(OPERATION, "Download parent directory could not be read")
    })?;
    let name = paths::basename(&normalized);
    Ok(!entries.iter().any(|entry| entry == name))
}
