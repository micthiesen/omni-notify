//! Audio capture through fake `yt-dlp` / `ffmpeg` executables (`sh -c`, no
//! network). The timeout case runs on
//! real time (25 ms) because tokio's paused clock cannot advance while a real
//! child process is awaited.
#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::time::Duration;

use omni_core::clock::{SharedClock, TestClock};
use omni_live_intel::audio::{AudioError, LivestreamAudioCapture, run_audio_process};

fn sh(script: &str) -> Vec<String> {
    vec!["-c".to_owned(), script.to_owned()]
}

#[tokio::test]
async fn classifies_a_deterministic_output_limit_as_non_retryable() {
    let result = run_audio_process("sh", &sh("printf overflow"), Duration::from_secs(2), 2).await;
    match result {
        Err(AudioError::Process { message, retryable }) => {
            assert!(!retryable);
            assert!(message.contains("exceeded output limit"), "{message}");
        }
        other => panic!("expected a process error, got {other:?}"),
    }
}

#[tokio::test]
async fn classifies_a_deterministic_timeout_as_non_retryable_and_interrupts_the_child() {
    let started = std::time::Instant::now();
    let result = run_audio_process("sh", &sh("sleep 5"), Duration::from_millis(25), 100).await;
    match result {
        Err(AudioError::Process { message, retryable }) => {
            assert!(!retryable);
            assert!(message.contains("timed out after 25ms"), "{message}");
        }
        other => panic!("expected a process error, got {other:?}"),
    }
    assert!(started.elapsed() < Duration::from_secs(4));
}

#[tokio::test]
async fn a_failing_exit_is_retryable_with_the_stderr_tail() {
    let result = run_audio_process(
        "sh",
        &sh("echo boom >&2; exit 3"),
        Duration::from_secs(5),
        100,
    )
    .await;
    match result {
        Err(AudioError::Process { message, retryable }) => {
            assert!(retryable);
            assert_eq!(message, "sh exited 3: boom\n");
        }
        other => panic!("expected a process error, got {other:?}"),
    }
}

fn write_script(path: &Path, body: &str) {
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).expect("write script");
    let mut perms = std::fs::metadata(path).expect("meta").permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).expect("chmod");
}

fn clock() -> SharedClock {
    TestClock::new(1_700_000_000_000)
}

#[tokio::test]
async fn captures_audio_through_fake_yt_dlp_and_ffmpeg() {
    let dir = tempfile::tempdir().expect("tempdir");
    let calls = dir.path().join("calls.log");
    let yt = dir.path().join("yt-dlp");
    let ff = dir.path().join("ffmpeg");
    write_script(
        &yt,
        &format!(
            "echo yt \"$@\" >> {calls}\nprintf '{{\"url\":\"https://media.example/a.m3u8\",\"http_headers\":{{\"Referer\":\"https://x\"}}}}'",
            calls = calls.display()
        ),
    );
    // Two seconds of 16 kHz f32le silence.
    write_script(
        &ff,
        &format!(
            "echo ff \"$@\" >> {calls}\nhead -c 128000 /dev/zero",
            calls = calls.display()
        ),
    );
    let capture =
        LivestreamAudioCapture::new(yt.display().to_string(), ff.display().to_string(), clock());
    let audio = capture
        .capture_seek("https://kick.com/someone", 2.0, None)
        .await
        .expect("captured");
    assert_eq!(audio.samples.len(), 32_000);
    assert_eq!(audio.sample_rate, 16_000);
    assert_eq!(audio.duration_seconds, 2.0);
    // Cached resolution: a second capture does not call yt-dlp again.
    capture
        .capture_seek("https://kick.com/someone", 2.0, Some(30.0))
        .await
        .expect("captured again");
    let log = std::fs::read_to_string(&calls).expect("calls");
    assert_eq!(log.lines().filter(|l| l.starts_with("yt ")).count(), 1);
    let yt_line = log.lines().find(|l| l.starts_with("yt ")).expect("yt call");
    assert!(yt_line.contains("--user-agent OpenAI File Downloader, XaiImageApiFetch/1.0"));
    assert!(yt_line.contains("--format worstaudio[language^=en]/worstaudio/best"));
    assert!(
        yt_line.ends_with("--print %(.{url,http_headers,live_status})j https://kick.com/someone")
    );
    // The -headers value ends in CRLF, so each ffmpeg call spans two log lines.
    assert!(log.contains("ff -hide_banner -loglevel error -headers Referer: https://x\r\n -i https://media.example/a.m3u8 -t 2 -vn -ac 1 -ar 16000 -f f32le pipe:1\n"));
    assert!(
        log.contains("ff -hide_banner -loglevel error -ss 30 -headers Referer: https://x\r\n -i ")
    );
}

#[tokio::test]
async fn too_little_audio_retries_once_with_a_fresh_resolution() {
    let dir = tempfile::tempdir().expect("tempdir");
    let calls = dir.path().join("calls.log");
    let yt = dir.path().join("yt-dlp");
    let ff = dir.path().join("ffmpeg");
    write_script(
        &yt,
        &format!(
            "echo yt >> {calls}\nprintf '{{\"url\":\"https://media.example/a\"}}'",
            calls = calls.display()
        ),
    );
    write_script(
        &ff,
        &format!(
            "echo ff >> {calls}\nhead -c 400 /dev/zero",
            calls = calls.display()
        ),
    );
    let capture =
        LivestreamAudioCapture::new(yt.display().to_string(), ff.display().to_string(), clock());
    let error = capture
        .capture_seek("https://kick.com/someone", 2.0, None)
        .await
        .expect_err("too short");
    assert_eq!(error.to_string(), "Captured less than one second of audio");
    let log = std::fs::read_to_string(&calls).expect("calls");
    assert_eq!(log.lines().filter(|l| *l == "yt").count(), 2);
    assert_eq!(log.lines().filter(|l| *l == "ff").count(), 2);
}

#[tokio::test]
async fn invalid_yt_dlp_json_is_a_decode_failure_without_retry() {
    let dir = tempfile::tempdir().expect("tempdir");
    let yt = dir.path().join("yt-dlp");
    write_script(&yt, "printf '{\"title\":\"no url\"}'");
    let capture = LivestreamAudioCapture::new(yt.display().to_string(), "ffmpeg", clock());
    let error = capture
        .capture_seek("https://kick.com/someone", 2.0, None)
        .await
        .expect_err("no url");
    assert!(matches!(error, AudioError::Decode { .. }));
    assert_eq!(error.to_string(), "yt-dlp returned no playable media URL");
}

#[tokio::test]
async fn an_ended_kick_stream_is_not_live_without_a_retry_or_ffmpeg() {
    let dir = tempfile::tempdir().expect("tempdir");
    let calls = dir.path().join("calls.log");
    let yt = dir.path().join("yt-dlp");
    let ff = dir.path().join("ffmpeg");
    write_script(
        &yt,
        &format!(
            "echo yt >> {calls}\necho 'ERROR: [kick:live] prsek: Failed to download m3u8 information: HTTP Error 404: Not Found (caused by <HTTPError 404: Not Found>)' >&2\nexit 1",
            calls = calls.display()
        ),
    );
    write_script(&ff, &format!("echo ff >> {calls}", calls = calls.display()));
    let capture =
        LivestreamAudioCapture::new(yt.display().to_string(), ff.display().to_string(), clock());
    let error = capture
        .capture_seek("https://kick.com/prsek", 2.0, None)
        .await
        .expect_err("ended");
    assert_eq!(
        error,
        AudioError::NotLive {
            detail: "[kick:live] prsek: Failed to download m3u8 information: HTTP Error 404: Not Found (caused by <HTTPError 404: Not Found>)".into()
        }
    );
    assert!(!error.is_retryable());
    let log = std::fs::read_to_string(&calls).expect("calls");
    assert_eq!(log, "yt\n");
}

#[tokio::test]
async fn an_offline_channel_and_an_ended_broadcast_are_not_live() {
    let dir = tempfile::tempdir().expect("tempdir");
    let yt = dir.path().join("yt-dlp");
    write_script(
        &yt,
        "echo 'ERROR: [twitch:stream] guest: The channel is not currently live' >&2\nexit 1",
    );
    let capture = LivestreamAudioCapture::new(yt.display().to_string(), "ffmpeg", clock());
    let error = capture
        .capture_seek("https://www.twitch.tv/guest", 2.0, None)
        .await
        .expect_err("offline");
    assert_eq!(
        error.to_string(),
        "Stream is not live: [twitch:stream] guest: The channel is not currently live"
    );

    write_script(
        &yt,
        "printf '{\"url\":\"https://media.example/vod.mpd\",\"live_status\":\"post_live\"}'",
    );
    let error = capture
        .capture_seek("https://www.youtube.com/watch?v=x", 2.0, None)
        .await
        .expect_err("post live");
    assert_eq!(
        error,
        AudioError::NotLive {
            detail: "yt-dlp reports post_live".into()
        }
    );
}

#[tokio::test]
async fn other_yt_dlp_failures_stay_retryable_process_errors() {
    let dir = tempfile::tempdir().expect("tempdir");
    let yt = dir.path().join("yt-dlp");
    write_script(
        &yt,
        "echo 'ERROR: [kick:live] guest: HTTP Error 403: Forbidden' >&2\nexit 1",
    );
    let capture = LivestreamAudioCapture::new(yt.display().to_string(), "ffmpeg", clock());
    let error = capture
        .capture_seek("https://kick.com/guest", 2.0, None)
        .await
        .expect_err("forbidden");
    assert!(error.is_retryable(), "{error:?}");
}
