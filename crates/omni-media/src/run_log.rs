//! The per-run markdown log under `LOGS_PATH/recommendations/` (mitools `LogFile`).

use std::path::{Path, PathBuf};

use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

use crate::error::IntegrationError;

/// `## heading` sections written in order; the first write truncates
/// (overwrite mode) and concurrent sections never interleave.
#[derive(Debug)]
pub struct RunLogFile {
    path: PathBuf,
    truncated: Mutex<bool>,
}

impl RunLogFile {
    /// Creates the parent directory; nothing is written yet.
    pub async fn create(path: PathBuf) -> Result<Self, IntegrationError> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| IntegrationError::new(format!("mkdir {}", parent.display()), e))?;
        }
        Ok(Self {
            path,
            truncated: Mutex::new(false),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Appends one `## heading` section.
    pub async fn section(&self, heading: &str, content: &str) -> Result<(), IntegrationError> {
        let mut truncated = self.truncated.lock().await;
        let mut options = tokio::fs::OpenOptions::new();
        options.create(true);
        if *truncated {
            options.append(true);
        } else {
            options.write(true).truncate(true);
        }
        let write_error =
            |e: std::io::Error| IntegrationError::new(format!("write {}", self.path.display()), e);
        let mut file = options.open(&self.path).await.map_err(write_error)?;
        file.write_all(format!("## {heading}\n\n{content}\n\n").as_bytes())
            .await
            .map_err(write_error)?;
        file.flush().await.map_err(write_error)?;
        *truncated = true;
        Ok(())
    }
}

/// An optional run log: sections are dropped when no `LOGS_PATH` is configured.
pub async fn section(
    log: Option<&RunLogFile>,
    heading: &str,
    content: &str,
) -> Result<(), IntegrationError> {
    match log {
        Some(log) => log.section(heading, content).await,
        None => Ok(()),
    }
}
