//! Per-email Markdown run log under `LOGS_PATH/calendar-events/` (mitools
//! `LogFile` in overwrite mode): `## heading` sections, the first write
//! truncates. Failures to write are warned about and never fail the pipeline.

use std::path::{Path, PathBuf};

use tokio::io::AsyncWriteExt as _;
use tokio::sync::Mutex;

const LOG: &str = "Main:CalendarEvents";

pub struct RunLogFile {
    path: PathBuf,
    /// Whether the first (truncating) write happened.
    started: Mutex<bool>,
}

/// `logTimestamp`: local `YYYY-MM-DDTHH-MM-SS`.
pub fn log_timestamp(now_ms: i64, tz: &jiff::tz::TimeZone) -> String {
    jiff::Timestamp::from_millisecond(now_ms)
        .map(|ts| {
            ts.to_zoned(tz.clone())
                .strftime("%Y-%m-%dT%H-%M-%S")
                .to_string()
        })
        .unwrap_or_default()
}

/// `codeBlock`: a fence that does not collide with the content.
pub fn code_block(content: &str, lang: Option<&str>) -> String {
    let mut fence = "```".to_owned();
    while content.contains(&fence) {
        fence.push('`');
    }
    format!("{fence}{}\n{content}\n{fence}", lang.unwrap_or_default())
}

impl RunLogFile {
    /// Creates the parent directory; nothing is written yet.
    pub async fn create(path: PathBuf) -> Option<Self> {
        if let Some(parent) = path.parent()
            && let Err(error) = tokio::fs::create_dir_all(parent).await
        {
            tracing::warn!(target: LOG, "Could not create log directory {}: {error}", parent.display());
            return None;
        }
        Some(Self {
            path,
            started: Mutex::new(false),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Appends one `## heading` section.
    pub async fn section(&self, heading: &str, content: &str) {
        let mut started = self.started.lock().await;
        let mut options = tokio::fs::OpenOptions::new();
        if *started {
            options.append(true).create(true);
        } else {
            options.write(true).create(true).truncate(true);
        }
        let text = format!("## {heading}\n\n{content}\n\n");
        let result = async {
            let mut file = options.open(&self.path).await?;
            file.write_all(text.as_bytes()).await?;
            file.flush().await
        }
        .await;
        match result {
            Ok(()) => *started = true,
            Err(error) => {
                tracing::warn!(target: LOG, "Could not write {}: {error}", self.path.display());
            }
        }
    }
}
