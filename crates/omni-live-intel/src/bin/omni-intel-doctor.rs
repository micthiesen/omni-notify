//! Captures a clip (or reads 16 kHz mono f32le audio from `--file`), transcribes
//! it and scores it against the voiceprint, with timings. `--attribute` runs the
//! rolling-summary path instead: diarize, label speakers, transcribe each turn.
//!
//! `omni-intel-doctor (--url URL [--seek SECONDS] [--duration SECONDS] | --file PATH)
//!  [--attribute] [--model-dir DIR] [--voiceprint PATH]`

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context as _, bail};
use omni_core::clock::SystemClock;
use omni_live_intel::audio::{CapturedAudio, LivestreamAudioCapture, SAMPLE_RATE, decode_f32le};
use omni_live_intel::sherpa::SherpaBackend;
use omni_live_intel::speech::{DEFAULT_SPEAKER_THRESHOLD, LocalSpeechRuntime};
use omni_live_intel::transcript::{TranscribedTurn, describe_speakers, format_attributed};
use serde_json::json;

const DEFAULT_MODEL_DIR: &str = "/app/assets/livestream-intelligence/models";

fn value_after<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    let index = args.iter().position(|a| a == flag)?;
    args.get(index + 1).map(String::as_str)
}

fn number(args: &[String], flag: &str, default: f64) -> anyhow::Result<f64> {
    let value = value_after(args, flag).map_or(default, omni_core::js::string_to_number);
    if !value.is_finite() || value < 0.0 {
        bail!("{flag} must be a non-negative number");
    }
    Ok(value)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let duration = number(&args, "--duration", 30.0)?;
    let seek = number(&args, "--seek", 0.0)?;
    let model_dir = PathBuf::from(value_after(&args, "--model-dir").map_or_else(
        || std::env::var("LIVESTREAM_MODEL_DIR").unwrap_or_else(|_| DEFAULT_MODEL_DIR.to_owned()),
        str::to_owned,
    ));
    let voiceprint = value_after(&args, "--voiceprint")
        .map(str::to_owned)
        .or_else(|| std::env::var("LIVESTREAM_DESTINY_VOICEPRINT_PATH").ok())
        .filter(|p| !p.is_empty())
        .map(PathBuf::from);

    let started = Instant::now();
    let audio = if let Some(file) = value_after(&args, "--file") {
        let samples = decode_f32le(&std::fs::read(file).context("read audio file")?);
        #[allow(clippy::cast_precision_loss)]
        let duration_seconds = samples.len() as f64 / f64::from(SAMPLE_RATE);
        CapturedAudio {
            samples,
            sample_rate: SAMPLE_RATE,
            duration_seconds,
        }
    } else {
        let Some(url) = value_after(&args, "--url") else {
            bail!(
                "parse arguments: Usage: (--url URL [--seek SECONDS] [--duration SECONDS] | --file PATH)"
            );
        };
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
        capture
            .capture_seek(url, duration, Some(seek))
            .await
            .context("capture audio")?
    };
    let captured = started.elapsed().as_secs_f64();
    let audio_seconds = audio.duration_seconds;
    let load = move || {
        omni_live_intel::load_speech_runtime(
            &model_dir,
            voiceprint.as_deref(),
            DEFAULT_SPEAKER_THRESHOLD,
        )
        .context("initialize speech runtime")
    };
    if args.iter().any(|a| a == "--attribute") {
        let report = tokio::task::spawn_blocking(move || attribute(&load()?, &audio)).await??;
        writeln!(
            std::io::stdout(),
            "{}",
            omni_core::js::json_stringify_pretty2(&report)
        )?;
        return Ok(());
    }
    let (transcript, matched, transcribed, finished) = tokio::task::spawn_blocking(move || {
        // The transcription timing includes loading the runtime.
        let transcription_started = Instant::now();
        let speech = load()?;
        let transcript = speech
            .transcribe(&audio.samples)
            .context("transcribe audio")?;
        let transcribed = transcription_started.elapsed().as_secs_f64();
        let speaker_started = Instant::now();
        let matched = speech
            .detect_destiny(&audio.samples)
            .context("detect speaker")?;
        anyhow::Ok((
            transcript,
            matched,
            transcribed,
            speaker_started.elapsed().as_secs_f64(),
        ))
    })
    .await??;
    let report = json!({
        "audioSeconds": audio_seconds,
        "captureSeconds": captured,
        "transcriptionSeconds": transcribed,
        "transcriptionRealtimeFactor": transcribed / audio_seconds,
        "speakerSeconds": finished,
        "match": {
            "confidence": matched.confidence,
            "matchedWindows": matched.matched_windows,
            "checkedWindows": matched.checked_windows,
        },
        "transcript": transcript,
    });
    writeln!(
        std::io::stdout(),
        "{}",
        omni_core::js::json_stringify_pretty2(&report)
    )?;
    Ok(())
}

/// The rolling-summary speech path, timed.
fn attribute(
    speech: &LocalSpeechRuntime<SherpaBackend>,
    audio: &CapturedAudio,
) -> anyhow::Result<serde_json::Value> {
    let diarize_started = Instant::now();
    let attribution = speech
        .attribute_speakers(&audio.samples)
        .context("attribute speakers")?;
    let diarized = diarize_started.elapsed().as_secs_f64();
    let transcribe_started = Instant::now();
    let mut turns = Vec::new();
    for turn in &attribution.turns {
        turns.push(TranscribedTurn {
            speaker: turn.speaker,
            text: speech
                .transcribe(&audio.samples[turn.start..turn.end])
                .context("transcribe turn")?,
        });
    }
    let transcribed = transcribe_started.elapsed().as_secs_f64();
    let speakers: Vec<serde_json::Value> = attribution
        .speakers
        .iter()
        .map(|p| {
            json!({
                "label": p.label,
                "seconds": p.seconds,
                "destinyConfidence": p.destiny_confidence,
            })
        })
        .collect();
    Ok(json!({
        "audioSeconds": audio.duration_seconds,
        "diarizationSeconds": diarized,
        "transcriptionSeconds": transcribed,
        "realtimeFactor": (diarized + transcribed) / audio.duration_seconds,
        "turns": attribution.turns.len(),
        "speakers": speakers,
        "speakerSummary": describe_speakers(&attribution.speakers),
        "transcript": format_attributed(&turns, &attribution.speakers),
    }))
}
