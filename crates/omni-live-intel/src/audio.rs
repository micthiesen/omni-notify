//! Bounded livestream audio capture through `yt-dlp` and `ffmpeg` subprocesses.
//!
//! The runner mirrors `runAudioProcess`: stdout over its cap or a timeout kills
//! the child and fails non-retryably, a non-zero exit fails retryably with the
//! stderr tail, and stderr is truncated (never an error) at 16 KiB. The shared
//! `omni_core::process::run_bounded` treats stderr overflow as an error, so it
//! is not used here.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use indexmap::IndexMap;
use omni_core::clock::SharedClock;
use serde::Deserialize;
use tokio::io::{AsyncRead, AsyncReadExt as _};
use tokio::process::Command;

use crate::USER_AGENT;

/// Mono 16 kHz, the rate every speech model expects.
pub const SAMPLE_RATE: u32 = 16_000;
const MAX_STDERR_BYTES: usize = 16_384;
const RESOLVE_CACHE_MS: i64 = 10 * 60_000;

/// Captured mono PCM.
#[derive(Clone, Debug, PartialEq)]
pub struct CapturedAudio {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub duration_seconds: f64,
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum AudioError {
    /// A subprocess failed. `retryable` is false for deterministic failures
    /// (output limit, timeout) that a fresh media URL would not fix.
    #[error("{message}")]
    Process { message: String, retryable: bool },
    /// yt-dlp output could not be decoded.
    #[error("{message}")]
    Decode { message: String, cause: String },
    /// The stream is not broadcasting: it ended (or has not started) while
    /// the live check still reports it live. Not a capture failure.
    #[error("Stream is not live: {detail}")]
    NotLive { detail: String },
}

impl AudioError {
    fn process(message: impl Into<String>, retryable: bool) -> Self {
        Self::Process {
            message: message.into(),
            retryable,
        }
    }

    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Process {
                retryable: true,
                ..
            }
        )
    }
}

/// yt-dlp failures that mean the stream is not broadcasting. Kick keeps the
/// channel's `livestream` for a while after a broadcast ends while its playlist
/// already returns 404, and the live check lags the platform by up to a minute.
const NOT_LIVE_MARKERS: &[&str] = &[
    "is not currently live",
    "Failed to download m3u8 information: HTTP Error 404",
    "This live event has ended",
    "This live event will begin",
    "Premieres in",
];

/// yt-dlp `live_status` values for media that is not broadcasting now.
const NOT_LIVE_STATUSES: &[&str] = &["is_upcoming", "post_live", "was_live", "not_live"];

/// Reclassifies a yt-dlp exit whose stderr says the stream is not live.
fn classify_resolve_error(error: AudioError) -> AudioError {
    match error {
        AudioError::Process { message, .. }
            if NOT_LIVE_MARKERS
                .iter()
                .any(|marker| message.contains(marker)) =>
        {
            let detail = message
                .rsplit_once("ERROR: ")
                .map_or(message.as_str(), |(_, tail)| tail)
                .trim()
                .to_owned();
            AudioError::NotLive { detail }
        }
        other => other,
    }
}

/// Process output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessOutput {
    pub stdout: Vec<u8>,
    pub stderr: String,
}

async fn read_stdout_capped<R: AsyncRead + Unpin>(
    mut pipe: R,
    limit: usize,
    command: &str,
) -> Result<Vec<u8>, AudioError> {
    let mut out = Vec::new();
    let mut chunk = vec![0_u8; 64 * 1024];
    loop {
        let read = pipe
            .read(&mut chunk)
            .await
            .map_err(|e| AudioError::process(e.to_string(), true))?;
        if read == 0 {
            return Ok(out);
        }
        if out.len() + read > limit {
            return Err(AudioError::process(
                format!("{command} exceeded output limit"),
                false,
            ));
        }
        out.extend_from_slice(&chunk[..read]);
    }
}

async fn read_stderr_truncated<R: AsyncRead + Unpin>(mut pipe: R) -> Vec<u8> {
    let mut out = Vec::new();
    let mut chunk = vec![0_u8; 4096];
    while let Ok(read) = pipe.read(&mut chunk).await {
        if read == 0 {
            break;
        }
        let remaining = MAX_STDERR_BYTES.saturating_sub(out.len());
        out.extend_from_slice(&chunk[..read.min(remaining)]);
    }
    out
}

/// Last 500 UTF-16 units of `text`, as `.slice(-500)`.
fn tail_500(text: &str) -> String {
    let len = omni_core::js::utf16_len(text);
    omni_core::js::utf16_slice(text, len.saturating_sub(500), len).into_owned()
}

/// Runs `command args` with stdin closed.
pub async fn run_audio_process(
    command: &str,
    args: &[String],
    timeout: Duration,
    max_stdout_bytes: usize,
) -> Result<ProcessOutput, AudioError> {
    let mut child = Command::new(command)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| AudioError::process(e.to_string(), true))?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let work = async {
        let stdout_task = async {
            match stdout {
                Some(pipe) => read_stdout_capped(pipe, max_stdout_bytes, command).await,
                None => Ok(Vec::new()),
            }
        };
        let stderr_task = async {
            match stderr {
                Some(pipe) => read_stderr_truncated(pipe).await,
                None => Vec::new(),
            }
        };
        // try_join: an output-limit failure returns at once, dropping the
        // child (and killing it) instead of waiting for stderr to close.
        let (stdout, stderr) = tokio::try_join!(stdout_task, async {
            Ok::<_, AudioError>(stderr_task.await)
        })?;
        let status = child
            .wait()
            .await
            .map_err(|e| AudioError::process(e.to_string(), true))?;
        let stderr = String::from_utf8_lossy(&stderr).into_owned();
        if status.success() {
            return Ok(ProcessOutput { stdout, stderr });
        }
        let code = status.code().map_or_else(
            || {
                #[cfg(unix)]
                {
                    use std::os::unix::process::ExitStatusExt as _;
                    status
                        .signal()
                        .map_or_else(|| "null".to_owned(), signal_name)
                }
                #[cfg(not(unix))]
                {
                    "null".to_owned()
                }
            },
            |code| code.to_string(),
        );
        Err(AudioError::process(
            format!("{command} exited {code}: {}", tail_500(&stderr)),
            true,
        ))
    };
    // Dropping `work` (timeout or cap error) drops the child: kill_on_drop.
    match tokio::time::timeout(timeout, work).await {
        Ok(result) => result,
        Err(_) => Err(AudioError::process(
            format!("{command} timed out after {}ms", timeout.as_millis()),
            false,
        )),
    }
}

#[cfg(unix)]
fn signal_name(signal: i32) -> String {
    match signal {
        1 => "SIGHUP",
        2 => "SIGINT",
        6 => "SIGABRT",
        9 => "SIGKILL",
        11 => "SIGSEGV",
        13 => "SIGPIPE",
        15 => "SIGTERM",
        _ => return format!("signal {signal}"),
    }
    .to_owned()
}

/// What yt-dlp resolved: a playable media URL and the headers it needs.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct ResolvedMedia {
    pub url: String,
    #[serde(default)]
    pub http_headers: Option<IndexMap<String, String>>,
    #[serde(default)]
    pub live_status: Option<String>,
}

/// yt-dlp prints only the fields the capture needs: a full `--dump-single-json`
/// of an ended YouTube stream (DASH fragment lists) can run past any sane cap.
const RESOLVE_TEMPLATE: &str = "%(.{url,http_headers,live_status})j";

/// `-headers` value: `Name: value\r\n` per non-empty header, newlines flattened.
fn header_argument(headers: Option<&IndexMap<String, String>>) -> Option<String> {
    let lines: Vec<String> = headers?
        .iter()
        .filter(|(name, value)| !name.is_empty() && !value.is_empty())
        .map(|(name, value)| format!("{name}: {}", value.replace(['\r', '\n'], " ")))
        .collect();
    (!lines.is_empty()).then(|| format!("{}\r\n", lines.join("\r\n")))
}

/// Capture seam for the service (fakes in tests).
pub trait AudioSource: Send + Sync + 'static {
    fn capture<'a>(
        &'a self,
        stream_url: &'a str,
        duration_seconds: u32,
    ) -> futures::future::BoxFuture<'a, Result<CapturedAudio, AudioError>>;
}

/// `LivestreamAudioCapture`: resolves with yt-dlp (cached ten minutes per URL)
/// and decodes a bounded window with ffmpeg.
#[derive(Clone)]
pub struct LivestreamAudioCapture {
    yt_dlp: String,
    ffmpeg: String,
    clock: SharedClock,
    cache: Arc<Mutex<HashMap<String, (ResolvedMedia, i64)>>>,
}

impl LivestreamAudioCapture {
    pub fn new(yt_dlp: impl Into<String>, ffmpeg: impl Into<String>, clock: SharedClock) -> Self {
        Self {
            yt_dlp: yt_dlp.into(),
            ffmpeg: ffmpeg.into(),
            clock,
            cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn cache(&self) -> std::sync::MutexGuard<'_, HashMap<String, (ResolvedMedia, i64)>> {
        self.cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    async fn resolve(&self, stream_url: &str) -> Result<ResolvedMedia, AudioError> {
        let now = self.clock.now_ms();
        if let Some((media, expires_at)) = self.cache().get(stream_url)
            && *expires_at > now
        {
            return Ok(media.clone());
        }
        let args: Vec<String> = [
            "--quiet",
            "--no-warnings",
            "--no-playlist",
            "--js-runtimes",
            "node",
            "--user-agent",
            USER_AGENT,
            "--format",
            "worstaudio[language^=en]/worstaudio/best",
            "--print",
            RESOLVE_TEMPLATE,
            stream_url,
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        let output = run_audio_process(
            &self.yt_dlp,
            &args,
            Duration::from_secs(30),
            4 * 1024 * 1024,
        )
        .await
        .map_err(classify_resolve_error)?;
        let raw: serde_json::Value =
            serde_json::from_slice(&output.stdout).map_err(|e| AudioError::Decode {
                message: "yt-dlp returned invalid JSON".to_owned(),
                cause: e.to_string(),
            })?;
        let media: ResolvedMedia = serde_json::from_value(raw).map_err(|e| AudioError::Decode {
            message: "yt-dlp returned no playable media URL".to_owned(),
            cause: e.to_string(),
        })?;
        if let Some(status) = media
            .live_status
            .as_deref()
            .filter(|status| NOT_LIVE_STATUSES.contains(status))
        {
            return Err(AudioError::NotLive {
                detail: format!("yt-dlp reports {status}"),
            });
        }
        self.cache().insert(
            stream_url.to_owned(),
            (media.clone(), now + RESOLVE_CACHE_MS),
        );
        Ok(media)
    }

    /// Captures `duration_seconds` from `stream_url`, optionally seeking first.
    /// A retryable failure clears the resolution cache and retries once.
    pub async fn capture_seek(
        &self,
        stream_url: &str,
        duration_seconds: f64,
        seek_seconds: Option<f64>,
    ) -> Result<CapturedAudio, AudioError> {
        let first = match self.resolve(stream_url).await {
            Ok(media) => {
                self.capture_resolved(&media, duration_seconds, seek_seconds)
                    .await
            }
            Err(e) => Err(e),
        };
        match first {
            Ok(audio) => Ok(audio),
            Err(error) => {
                self.cache().remove(stream_url);
                if !error.is_retryable() {
                    return Err(error);
                }
                let media = self.resolve(stream_url).await?;
                self.capture_resolved(&media, duration_seconds, seek_seconds)
                    .await
            }
        }
    }

    async fn capture_resolved(
        &self,
        media: &ResolvedMedia,
        duration_seconds: f64,
        seek_seconds: Option<f64>,
    ) -> Result<CapturedAudio, AudioError> {
        let mut args: Vec<String> = ["-hide_banner", "-loglevel", "error"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        if let Some(seek) = seek_seconds.filter(|s| *s > 0.0) {
            args.push("-ss".to_owned());
            args.push(omni_core::js::number_to_string(seek));
        }
        if let Some(headers) = header_argument(media.http_headers.as_ref()) {
            args.push("-headers".to_owned());
            args.push(headers);
        }
        for arg in [
            "-i",
            &media.url,
            "-t",
            &omni_core::js::number_to_string(duration_seconds),
            "-vn",
            "-ac",
            "1",
            "-ar",
            &SAMPLE_RATE.to_string(),
            "-f",
            "f32le",
            "pipe:1",
        ] {
            args.push(arg.to_owned());
        }
        let seconds = duration_seconds;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let max_bytes = (seconds * f64::from(SAMPLE_RATE) * 4.0 * 1.1).ceil() as usize;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let timeout_ms = (seconds * 2_000.0 + 15_000.0).max(30_000.0) as u64;
        let output = run_audio_process(
            &self.ffmpeg,
            &args,
            Duration::from_millis(timeout_ms),
            max_bytes,
        )
        .await?;
        let samples = decode_f32le(&output.stdout);
        if samples.len() < SAMPLE_RATE as usize {
            return Err(AudioError::process(
                "Captured less than one second of audio",
                true,
            ));
        }
        #[allow(clippy::cast_precision_loss)]
        let duration_seconds = samples.len() as f64 / f64::from(SAMPLE_RATE);
        Ok(CapturedAudio {
            samples,
            sample_rate: SAMPLE_RATE,
            duration_seconds,
        })
    }
}

impl AudioSource for LivestreamAudioCapture {
    fn capture<'a>(
        &'a self,
        stream_url: &'a str,
        duration_seconds: u32,
    ) -> futures::future::BoxFuture<'a, Result<CapturedAudio, AudioError>> {
        Box::pin(self.capture_seek(stream_url, f64::from(duration_seconds), None))
    }
}

/// Little-endian f32 samples; a trailing partial sample is dropped.
pub fn decode_f32le(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_argument_flattens_newlines_and_skips_empty() {
        let mut headers = IndexMap::new();
        headers.insert("User-Agent".to_owned(), "a\r\nb".to_owned());
        headers.insert("Empty".to_owned(), String::new());
        assert_eq!(
            header_argument(Some(&headers)).as_deref(),
            Some("User-Agent: a  b\r\n")
        );
        assert_eq!(header_argument(None), None);
    }

    #[test]
    fn decodes_aligned_samples_only() {
        let mut bytes = 0.5_f32.to_le_bytes().to_vec();
        bytes.extend_from_slice(&[1, 2]);
        assert_eq!(decode_f32le(&bytes), vec![0.5]);
    }
}
