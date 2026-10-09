//! `omni-notify doctor [--image]`: runtime invariants of the image
//! (ARCHITECTURE.md section 8). The Docker build runs `doctor --image`, which
//! fails the build when any check fails; without `--image` the checks are
//! the same but reported for a local install.

use std::path::{Path, PathBuf};
use std::time::Duration;

use omni_config::Config;

/// One check's outcome.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

impl Check {
    fn new(name: impl Into<String>, ok: bool, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ok,
            detail: detail.into(),
        }
    }
}

const OUTPUT_CAP: usize = 4 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(30);

async fn command_output(program: &str, args: &[&str]) -> Result<String, String> {
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args);
    let output = omni_core::process::run_bounded(cmd, None, OUTPUT_CAP, OUTPUT_CAP, TIMEOUT)
        .await
        .map_err(|e| e.to_string())?;
    if output.status != 0 {
        return Err(format!("exit status {}", output.status));
    }
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    Ok(text)
}

/// `true` when `listing` (ffmpeg `-filters` / `-encoders`) names `name` as a column.
pub fn lists(listing: &str, name: &str) -> bool {
    listing
        .lines()
        .any(|line| line.split_whitespace().nth(1) == Some(name))
}

fn file_check(path: &Path) -> Check {
    Check::new(
        path.display().to_string(),
        path.is_file(),
        if path.is_file() { "present" } else { "missing" },
    )
}

/// Runs every check.
pub async fn checks(config: &Config, assets_dir: &Path) -> Vec<Check> {
    let mut out = Vec::new();
    let ffmpeg = config.ffmpeg_bin();
    match command_output(ffmpeg, &["-hide_banner", "-filters"]).await {
        Ok(listing) => {
            for filter in ["arnndn", "firequalizer"] {
                out.push(Check::new(
                    format!("ffmpeg filter {filter}"),
                    lists(&listing, filter),
                    ffmpeg,
                ));
            }
        }
        Err(error) => out.push(Check::new("ffmpeg -filters", false, error)),
    }
    match command_output(ffmpeg, &["-hide_banner", "-encoders"]).await {
        Ok(listing) => out.push(Check::new(
            "ffmpeg encoder libmp3lame",
            lists(&listing, "libmp3lame"),
            ffmpeg,
        )),
        Err(error) => out.push(Check::new("ffmpeg -encoders", false, error)),
    }
    out.push(file_check(&assets_dir.join("press-pods/denoise.rnnn")));
    for path in [
        omni_personal::printer::service::BRLASER_FILTER,
        omni_personal::printer::service::CUPS_FILTER,
        omni_personal::printer::service::BRLASER_PPD,
    ] {
        out.push(file_check(Path::new(path)));
    }
    out.push(match command_output("pdfinfo", &["-v"]).await {
        Ok(_) => Check::new("pdfinfo -v", true, "runs"),
        Err(error) => Check::new("pdfinfo -v", false, error),
    });
    let models =
        omni_live_intel::speech::ModelFiles::in_dir(&PathBuf::from(&config.livestream_model_dir));
    for path in models.all() {
        out.push(file_check(path));
    }
    let yt_dlp = config.yt_dlp_bin();
    out.push(match command_output(yt_dlp, &["--version"]).await {
        Ok(version) => Check::new("yt-dlp --version", true, version.trim().to_owned()),
        Err(error) => Check::new("yt-dlp --version", false, error),
    });
    out
}

/// A report line per check.
pub fn report(checks: &[Check]) -> String {
    checks
        .iter()
        .map(|c| {
            format!(
                "{} {}: {}\n",
                if c.ok { "ok  " } else { "FAIL" },
                c.name,
                c.detail
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_filters_in_ffmpeg_listings() {
        let listing = " ... arnndn            A->A       Reduce noise from speech using Recurrent Neural Networks.\n T.. firequalizer      A->A       Finite Impulse Response Equalizer.\n";
        assert!(lists(listing, "arnndn"));
        assert!(lists(listing, "firequalizer"));
        assert!(!lists(listing, "afftdn"));
        let encoders =
            " A....D libmp3lame           libmp3lame MP3 (MPEG audio layer 3) (codec mp3)\n";
        assert!(lists(encoders, "libmp3lame"));
    }
}
