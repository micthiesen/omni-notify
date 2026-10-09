//! Markdown run logs under `LOGS_PATH/briefings/` (mitools `LogFile` in
//! overwrite mode plus `codeBlock`).

use std::path::{Path, PathBuf};

use tokio::io::AsyncWriteExt as _;
use tokio::sync::Mutex;

/// A `## heading` sectioned file; the first write truncates, later ones append,
/// and writes are serialized.
#[derive(Debug)]
pub struct LogFile {
    path: PathBuf,
    truncated: Mutex<bool>,
}

impl LogFile {
    /// Creates the parent directory; nothing is written yet.
    pub async fn make(path: PathBuf) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
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
    pub async fn section(&self, heading: &str, content: &str) -> std::io::Result<()> {
        let mut truncated = self.truncated.lock().await;
        let mut options = tokio::fs::OpenOptions::new();
        options.create(true);
        if *truncated {
            options.append(true);
        } else {
            options.write(true).truncate(true);
        }
        let mut file = options.open(&self.path).await?;
        file.write_all(format!("## {heading}\n\n{content}\n\n").as_bytes())
            .await?;
        file.flush().await?;
        *truncated = true;
        Ok(())
    }
}

/// A fenced code block whose fence cannot collide with `content`.
pub fn code_block(content: &str, lang: Option<&str>) -> String {
    let mut fence = String::from("```");
    while content.contains(&fence) {
        fence.push('`');
    }
    format!("{fence}{}\n{content}\n{fence}", lang.unwrap_or(""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fences_never_collide() {
        assert_eq!(code_block("x", None), "```\nx\n```");
        assert_eq!(
            code_block("a ``` b", Some("json")),
            "````json\na ``` b\n````"
        );
    }

    #[tokio::test]
    async fn first_section_truncates_then_appends() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/log.md");
        tokio::fs::create_dir_all(path.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&path, "stale").await.unwrap();
        let log = LogFile::make(path.clone()).await.unwrap();
        log.section("One", "a").await.unwrap();
        log.section("Two", "b").await.unwrap();
        assert_eq!(
            tokio::fs::read_to_string(&path).await.unwrap(),
            "## One\n\na\n\n## Two\n\nb\n\n"
        );
    }
}
