//! Markdown diagnostic log files under `LOGS_PATH/parcel-tracker`:
//! `## heading` sections, serialized writes, append or
//! overwrite-on-first-write. Diagnostics never fail processing: write errors
//! are warned about and dropped.

use std::path::{Path, PathBuf};

use tokio::io::AsyncWriteExt as _;
use tokio::sync::Mutex;

const LOG: &str = "Main:ParcelTracker";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogFileMode {
    Append,
    /// The first write truncates; later writes append.
    Overwrite,
}

pub struct LogFile {
    path: PathBuf,
    mode: LogFileMode,
    /// Serializes writes; `true` once an overwrite-mode file was truncated.
    truncated: Mutex<bool>,
}

impl LogFile {
    /// Creates the parent directory; nothing is written yet.
    pub async fn make(path: impl Into<PathBuf>, mode: LogFileMode) -> std::io::Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        Ok(Self {
            path,
            mode,
            truncated: Mutex::new(false),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Appends one `## heading` section.
    pub async fn section(&self, heading: &str, content: &str) {
        let mut truncated = self.truncated.lock().await;
        let truncate = self.mode == LogFileMode::Overwrite && !*truncated;
        let mut options = tokio::fs::OpenOptions::new();
        options.create(true);
        if truncate {
            options.write(true).truncate(true);
        } else {
            options.append(true);
        }
        let written = async {
            let mut file = options.open(&self.path).await?;
            file.write_all(format!("## {heading}\n\n{content}\n\n").as_bytes())
                .await?;
            file.flush().await
        }
        .await;
        match written {
            Ok(()) => *truncated = true,
            Err(error) => tracing::warn!(
                target: LOG,
                "Could not write {}: {error}",
                self.path.display()
            ),
        }
    }
}

/// A fence that does not collide with the content.
pub fn code_block(content: &str, lang: Option<&str>) -> String {
    let mut fence = "```".to_owned();
    while content.contains(&fence) {
        fence.push('`');
    }
    format!("{fence}{}\n{content}\n{fence}", lang.unwrap_or_default())
}

pub use omni_core::clock::log_timestamp;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fences_avoid_content_collisions() {
        assert_eq!(code_block("a", None), "```\na\n```");
        assert_eq!(code_block("x```y", Some("json")), "````json\nx```y\n````");
    }

    #[test]
    fn formats_local_timestamps() {
        let tz = jiff::tz::TimeZone::get("America/Vancouver").unwrap();
        assert_eq!(log_timestamp(1_773_671_405_000, &tz), "2026-03-16T07-30-05");
    }

    #[tokio::test]
    async fn overwrite_mode_truncates_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/run.md");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "stale").unwrap();
        let file = LogFile::make(&path, LogFileMode::Overwrite).await.unwrap();
        file.section("One", "a").await;
        file.section("Two", "b").await;
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "## One\n\na\n\n## Two\n\nb\n\n"
        );
    }
}
