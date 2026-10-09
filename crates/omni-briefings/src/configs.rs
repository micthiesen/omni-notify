//! Briefing configs: one `<Name>.md` per briefing under `BRIEFINGS_PATH`, with a
//! `schedule` in its YAML front matter and the prompt as its body
//! (`src/briefing-agent/configs.ts`).

use std::path::{Path, PathBuf};

use jiff::tz::TimeZone;
use omni_tasks::CronSchedule;
use serde::Deserialize;

const LOG: &str = "Briefings";

/// One loaded briefing.
#[derive(Clone, Debug)]
pub struct BriefingConfig {
    pub name: String,
    pub schedule: CronSchedule,
    pub prompt: String,
}

#[derive(Debug, thiserror::Error)]
#[error("read briefings folder {path}: {source}")]
pub struct ConfigLoadError {
    pub path: PathBuf,
    #[source]
    pub source: std::io::Error,
}

#[derive(Deserialize)]
struct FrontMatter {
    schedule: String,
}

/// `gray-matter` with the default `---` delimiters: the raw matter block (if
/// any) and the content after it. Mirrors `parseMatter`: a non-blank rest of
/// the opening line is a language tag; the block ends at the first `\n---`;
/// the content starts right after those four characters, minus one `\r` and
/// one `\n`.
pub fn split_front_matter(raw: &str) -> (Option<&str>, &str) {
    const OPEN: &str = "---";
    const CLOSE: &str = "\n---";
    if !raw.starts_with(OPEN) || raw[OPEN.len()..].starts_with('-') {
        return (None, raw);
    }
    let rest = &raw[OPEN.len()..];
    let Some(newline) = rest.find('\n') else {
        return (Some(""), "");
    };
    let line_end = if rest[..newline].ends_with('\r') {
        newline - 1
    } else {
        newline
    };
    let rest = if rest[..line_end].trim().is_empty() {
        rest
    } else {
        &rest[line_end..]
    };
    let Some(close) = rest.find(CLOSE) else {
        return (Some(rest), "");
    };
    let content = &rest[close + CLOSE.len()..];
    let content = content.strip_prefix('\r').unwrap_or(content);
    let content = content.strip_prefix('\n').unwrap_or(content);
    (Some(&rest[..close]), content)
}

/// Why a file was skipped (logged as a warning).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Skip {
    MissingSchedule,
    InvalidCron(String),
    EmptyBody,
}

/// Parses one briefing file.
pub fn parse_briefing(name: &str, raw: &str, tz: &TimeZone) -> Result<BriefingConfig, Skip> {
    let (matter, content) = split_front_matter(raw);
    let front: Option<FrontMatter> = matter
        .filter(|m| !m.trim().is_empty())
        .and_then(|m| serde_norway::from_str(m).ok());
    let schedule = front.ok_or(Skip::MissingSchedule)?.schedule;
    let schedule = CronSchedule::parse(&schedule, tz).map_err(|_| Skip::InvalidCron(schedule))?;
    let prompt = content.trim();
    if prompt.is_empty() {
        return Err(Skip::EmptyBody);
    }
    Ok(BriefingConfig {
        name: name.to_owned(),
        schedule,
        prompt: prompt.to_owned(),
    })
}

/// `loadBriefingConfigs`. Files are read in name order (node's `readdirSync`
/// order is the filesystem's).
pub fn load_briefing_configs(
    briefings_path: Option<&str>,
    tz: &TimeZone,
) -> Result<Vec<BriefingConfig>, ConfigLoadError> {
    let Some(briefings_path) = briefings_path.filter(|p| !p.is_empty()) else {
        tracing::info!(target: LOG, "No BRIEFINGS_PATH configured, skipping briefing tasks");
        return Ok(Vec::new());
    };
    let dir = Path::new(briefings_path);
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            tracing::warn!(target: LOG, "Briefings folder not found: {briefings_path}");
            return Ok(Vec::new());
        }
        Err(source) => {
            return Err(ConfigLoadError {
                path: dir.to_owned(),
                source,
            });
        }
    };
    let mut files: Vec<String> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| ConfigLoadError {
            path: dir.to_owned(),
            source,
        })?;
        if let Some(name) = entry.file_name().to_str()
            && name.ends_with(".md")
        {
            files.push(name.to_owned());
        }
    }
    files.sort();

    let mut configs = Vec::new();
    for file in files {
        let path = dir.join(&file);
        let raw = std::fs::read_to_string(&path).map_err(|source| ConfigLoadError {
            path: path.clone(),
            source,
        })?;
        let name = file.trim_end_matches(".md");
        match parse_briefing(name, &raw, tz) {
            Ok(config) => configs.push(config),
            Err(Skip::MissingSchedule) => {
                tracing::warn!(target: LOG, "Skipping {file}: missing or invalid 'schedule' field");
            }
            Err(Skip::InvalidCron(schedule)) => {
                tracing::warn!(target: LOG, "Skipping {file}: invalid cron expression \"{schedule}\"");
            }
            Err(Skip::EmptyBody) => {
                tracing::warn!(target: LOG, "Skipping {file}: empty body");
            }
        }
    }
    tracing::info!(
        target: LOG,
        "Loaded {} briefing config(s) from {briefings_path}",
        configs.len()
    );
    Ok(configs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_front_matter_like_gray_matter() {
        assert_eq!(
            split_front_matter("---\nschedule: \"x\"\n---\nBody\nmore"),
            (Some("\nschedule: \"x\""), "Body\nmore")
        );
        assert_eq!(split_front_matter("No matter"), (None, "No matter"));
        assert_eq!(split_front_matter("---\n---\nBody"), (Some(""), "Body"));
        assert_eq!(split_front_matter("---\na: 1\n---"), (Some("\na: 1"), ""));
        assert_eq!(split_front_matter("----\nx"), (None, "----\nx"));
        // The rest of the closing line belongs to the content.
        assert_eq!(
            split_front_matter("---\na: 1\n--- tail\r\nBody"),
            (Some("\na: 1"), " tail\r\nBody")
        );
        assert_eq!(
            split_front_matter("---\r\na: 1\r\n---\r\nBody"),
            (Some("\r\na: 1\r"), "Body")
        );
        // A language tag on the opening line is dropped.
        assert_eq!(
            split_front_matter("---yaml\na: 1\n---\nBody"),
            (Some("\na: 1"), "Body")
        );
    }
}
