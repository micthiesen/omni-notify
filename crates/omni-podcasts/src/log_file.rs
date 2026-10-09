//! mitools `LogFile`: a markdown run log of `## heading` sections, written
//! serially (overwrite mode truncates on the first section). Used for the
//! per-run `LOGS_PATH/podcast-recs/<local timestamp>.md` file.

use std::path::{Path, PathBuf};

use tokio::io::AsyncWriteExt as _;
use tokio::sync::Mutex;

#[derive(Debug, thiserror::Error)]
#[error("logfile: {operation} failed: {source}")]
pub struct LogFileError {
    pub operation: String,
    #[source]
    pub source: std::io::Error,
}

#[derive(Debug)]
pub struct LogFile {
    path: PathBuf,
    truncated: Mutex<bool>,
}

impl LogFile {
    /// Creates the parent directory; nothing is written yet.
    pub async fn create(path: PathBuf) -> Result<Self, LogFileError> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|source| LogFileError {
                    operation: format!("mkdir {}", parent.display()),
                    source,
                })?;
        }
        Ok(Self {
            path,
            truncated: Mutex::new(false),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Appends one section (the first write truncates).
    pub async fn section(&self, heading: &str, content: &str) -> Result<(), LogFileError> {
        let mut truncated = self.truncated.lock().await;
        let mut options = tokio::fs::OpenOptions::new();
        options.create(true);
        if *truncated {
            options.append(true);
        } else {
            options.write(true).truncate(true);
        }
        let error = |source| LogFileError {
            operation: format!("write {}", self.path.display()),
            source,
        };
        let mut file = options.open(&self.path).await.map_err(error)?;
        file.write_all(format!("## {heading}\n\n{content}\n\n").as_bytes())
            .await
            .map_err(error)?;
        file.flush().await.map_err(error)?;
        *truncated = true;
        Ok(())
    }
}

/// Writes a section when a log file is configured.
pub async fn section(
    log_file: Option<&LogFile>,
    heading: &str,
    content: &str,
) -> Result<(), LogFileError> {
    match log_file {
        Some(file) => file.section(heading, content).await,
        None => Ok(()),
    }
}

/// mitools `codeBlock`: a fence that cannot collide with the content.
pub fn code_block(content: &str, lang: Option<&str>) -> String {
    let mut fence = "```".to_owned();
    while content.contains(&fence) {
        fence.push('`');
    }
    format!("{fence}{}\n{content}\n{fence}", lang.unwrap_or_default())
}
