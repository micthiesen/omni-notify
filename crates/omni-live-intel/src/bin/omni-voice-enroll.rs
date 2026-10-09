//! Enrolls Destiny's voiceprint from two or more clips
//! (`src/tools/enroll-destiny-voice.ts`).
//!
//! `omni-voice-enroll --source URL [--seek SECONDS] --source URL [...]
//!  [--output PATH] [--model-dir DIR]`

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context as _, bail};
use omni_core::clock::{Clock as _, SystemClock};
use omni_live_intel::audio::{LivestreamAudioCapture, SAMPLE_RATE};
use omni_live_intel::enroll::{
    MIN_ENROLLMENT_WINDOWS, SourcedEmbedding, select_cross_source_cluster,
};
use omni_live_intel::js_math::js_to_fixed;
use omni_live_intel::speech::{DEFAULT_SPEAKER_THRESHOLD, VOICEPRINT_MODEL, VoiceprintFile};

const DEFAULT_OUTPUT: &str = "/data/livestream-intelligence/destiny.json";
const DEFAULT_MODEL_DIR: &str = "/app/assets/livestream-intelligence/models";
const CLIP_SECONDS: f64 = 60.0;
const WINDOW_SAMPLES: usize = 4 * SAMPLE_RATE as usize;

struct Source {
    url: String,
    seek: f64,
}

struct Args {
    output: PathBuf,
    model_dir: PathBuf,
    sources: Vec<Source>,
}

fn parse_args(args: impl Iterator<Item = String>) -> anyhow::Result<Args> {
    let mut output = PathBuf::from(DEFAULT_OUTPUT);
    let mut model_dir = PathBuf::from(
        std::env::var("LIVESTREAM_MODEL_DIR").unwrap_or_else(|_| DEFAULT_MODEL_DIR.to_owned()),
    );
    let mut sources: Vec<Source> = Vec::new();
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--output" => {
                if let Some(value) = args.next() {
                    output = PathBuf::from(value);
                }
            }
            "--model-dir" => {
                if let Some(value) = args.next() {
                    model_dir = PathBuf::from(value);
                }
            }
            "--source" => {
                let url = args.next().context("--source requires a URL")?;
                sources.push(Source { url, seek: 0.0 });
            }
            "--seek" => {
                let source = sources
                    .last_mut()
                    .context("--seek must follow a --source")?;
                let seek = omni_core::js::string_to_number(&args.next().unwrap_or_default());
                if !seek.is_finite() || seek < 0.0 {
                    bail!("--seek must be a non-negative number");
                }
                source.seek = seek;
            }
            other => bail!("Unknown argument: {other}"),
        }
    }
    if sources.len() < 2 {
        bail!("At least two --source clips are required");
    }
    Ok(Args {
        output,
        model_dir,
        sources,
    })
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = parse_args(std::env::args().skip(1)).context("parse arguments")?;
    let model_dir = args.model_dir.clone();
    let speech = tokio::task::spawn_blocking(move || {
        omni_live_intel::load_speech_runtime(&model_dir, None, DEFAULT_SPEAKER_THRESHOLD)
    })
    .await?
    .context("initialize speech runtime")?;
    let speech = Arc::new(speech);
    let capture = LivestreamAudioCapture::new(
        std::env::var("YT_DLP_PATH")
            .ok()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "yt-dlp".into()),
        std::env::var("FFMPEG_PATH")
            .ok()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "ffmpeg".into()),
        Arc::new(SystemClock),
    );
    let mut stdout = std::io::stdout();
    let mut embeddings = Vec::new();
    for (source_index, source) in args.sources.iter().enumerate() {
        let audio = capture
            .capture_seek(&source.url, CLIP_SECONDS, Some(source.seek))
            .await
            .context("capture audio")?;
        let runtime = Arc::clone(&speech);
        let samples = audio.samples;
        let windows = tokio::task::spawn_blocking(move || {
            let segments = runtime.extract_speech(&samples).context("extract speech")?;
            let mut windows = Vec::new();
            for segment in segments {
                let mut offset = 0;
                while offset + WINDOW_SAMPLES <= segment.len() {
                    windows.push(
                        runtime
                            .compute_embedding(&segment[offset..offset + WINDOW_SAMPLES])
                            .context("compute embedding")?,
                    );
                    offset += WINDOW_SAMPLES;
                }
            }
            anyhow::Ok(windows)
        })
        .await??;
        writeln!(
            stdout,
            "Captured {}s from {}, {} speech windows",
            js_to_fixed(audio.duration_seconds, 1),
            source.url,
            windows.len()
        )?;
        embeddings.extend(windows.into_iter().map(|embedding| SourcedEmbedding {
            embedding,
            source_index,
        }));
    }
    let selected = select_cross_source_cluster(&embeddings);
    if selected.len() < MIN_ENROLLMENT_WINDOWS {
        bail!(
            "select enrollment windows: Only {} consistent speech windows found; need at least {MIN_ENROLLMENT_WINDOWS}",
            selected.len()
        );
    }
    let count = selected.len();
    #[allow(clippy::cast_precision_loss)]
    let voiceprint = VoiceprintFile {
        version: 1.0,
        speaker: "destiny".to_owned(),
        model: VOICEPRINT_MODEL.to_owned(),
        embeddings: selected
            .into_iter()
            .map(|e| e.into_iter().map(f64::from).collect())
            .collect(),
        created_at: SystemClock.now_ms() as f64,
        sources: args
            .sources
            .iter()
            .map(|s| format!("{}#t={}", s.url, omni_core::js::number_to_string(s.seek)))
            .collect(),
    };
    if let Some(parent) = args.output.parent() {
        std::fs::create_dir_all(parent).context("create output directory")?;
    }
    write_private(&args.output, voiceprint.to_json_line()?.as_bytes())
        .context("write voiceprint")?;
    writeln!(
        stdout,
        "Wrote {count} enrollment embeddings to {}",
        args.output.display()
    )?;
    Ok(())
}

#[cfg(unix)]
fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)
}

#[cfg(not(unix))]
fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, bytes)
}
