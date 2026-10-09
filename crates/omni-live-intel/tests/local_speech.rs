//! Port of `src/live-check/intelligence/localSpeech.spec.ts`, plus the speaker
//! windowing and threshold logic over a fake backend (the native sherpa models
//! are not available to tests).
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::Path;
use std::sync::Mutex;

use omni_live_intel::load_speech_runtime;
use omni_live_intel::speech::{
    DEFAULT_SPEAKER_THRESHOLD, LocalSpeechRuntime, ModelFiles, SpeechBackend,
    SpeechRecognitionError, VoiceprintFile, cosine_similarity,
};

fn speech_fixture(voiceprint: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let files = ModelFiles::in_dir(dir.path());
    for path in files.all() {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, "").expect("write model fixture");
    }
    let voiceprint_path = dir.path().join("destiny.json");
    std::fs::write(&voiceprint_path, voiceprint).expect("write voiceprint");
    (dir, voiceprint_path)
}

#[test]
fn returns_a_typed_failure_for_malformed_voiceprint_json() {
    let (dir, voiceprint) = speech_fixture("{not-json");
    let error = load_speech_runtime(dir.path(), Some(&voiceprint), DEFAULT_SPEAKER_THRESHOLD)
        .err()
        .expect("fails");
    assert!(error.operation.contains("parse voiceprint"), "{error}");
}

#[test]
fn rejects_a_structurally_invalid_voiceprint_before_native_initialization() {
    let (dir, voiceprint) = speech_fixture(
        r#"{"version":1,"speaker":"destiny","model":"test","embeddings":[[0.1,0.2]],"createdAt":1,"sources":["test"]}"#,
    );
    let error = load_speech_runtime(dir.path(), Some(&voiceprint), DEFAULT_SPEAKER_THRESHOLD)
        .err()
        .expect("fails");
    assert!(error.operation.contains("decode voiceprint"), "{error}");
}

#[test]
fn missing_model_files_fail_validation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let error = load_speech_runtime(dir.path(), None, DEFAULT_SPEAKER_THRESHOLD)
        .err()
        .expect("fails");
    assert_eq!(error.operation, "validate livestream speech model files");
}

/// Segments are given; each window's embedding is `[window index, 1]`.
struct FakeBackend {
    segments: Vec<Vec<f32>>,
    windows: Mutex<Vec<usize>>,
}

impl SpeechBackend for FakeBackend {
    fn speech_segments(&self, _samples: &[f32]) -> Result<Vec<Vec<f32>>, SpeechRecognitionError> {
        Ok(self.segments.clone())
    }

    fn embedding(&self, samples: &[f32]) -> Result<Vec<f32>, SpeechRecognitionError> {
        self.windows.lock().expect("lock").push(samples.len());
        Ok(vec![samples[0], 1.0])
    }

    fn transcribe(&self, _samples: &[f32]) -> Result<String, SpeechRecognitionError> {
        Ok("  hello there  ".to_owned())
    }
}

fn voiceprint(embeddings: Vec<Vec<f64>>) -> VoiceprintFile {
    VoiceprintFile {
        version: 1.0,
        speaker: "destiny".into(),
        model: "3dspeaker-campplus-en-voxceleb-16k".into(),
        embeddings,
        created_at: 1.0,
        sources: vec![],
    }
}

#[test]
fn scores_four_second_windows_with_a_two_second_stride() {
    // A 9 s segment yields windows at 0, 2, 4 s (3 windows); a 3 s one yields none.
    let mut long = vec![0.0_f32; 9 * 16_000];
    long[0] = 1.0; // window 0 -> [1, 1]
    long[2 * 16_000] = -1.0; // window 1 -> [-1, 1]
    long[4 * 16_000] = 0.0; // window 2 -> [0, 1]
    let backend = FakeBackend {
        segments: vec![long, vec![0.0; 3 * 16_000]],
        windows: Mutex::new(vec![]),
    };
    let runtime = LocalSpeechRuntime::with_backend(
        backend,
        Some(voiceprint(vec![vec![1.0, 1.0], vec![0.0, -1.0]])),
        DEFAULT_SPEAKER_THRESHOLD,
    );
    let matched = runtime.detect_destiny(&[]).expect("detects");
    assert_eq!(matched.checked_windows, 3);
    // Scores: 1.0, 0.0 (best of 0 and -1), 0.7071 -> two at or above 0.62.
    assert_eq!(matched.matched_windows, 2);
    assert!((matched.confidence - 1.0).abs() < 1e-9);
    assert_eq!(runtime.transcribe(&[]).expect("text"), "hello there");
}

#[test]
fn without_a_voiceprint_nothing_is_checked() {
    let backend = FakeBackend {
        segments: vec![vec![0.0; 5 * 16_000]],
        windows: Mutex::new(vec![]),
    };
    let runtime = LocalSpeechRuntime::with_backend(backend, None, DEFAULT_SPEAKER_THRESHOLD);
    let matched = runtime.detect_destiny(&[]).expect("detects");
    assert_eq!(matched.checked_windows, 0);
    assert_eq!(matched.confidence, 0.0);
    assert!(!runtime.has_voiceprint());
}

#[test]
fn cosine_similarity_matches_the_ts_edge_cases() {
    assert_eq!(cosine_similarity(&[], &[]), -1.0);
    assert_eq!(cosine_similarity(&[1.0], &[1.0, 2.0]), -1.0);
    assert!((cosine_similarity(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-12);
    assert_eq!(cosine_similarity(&[0.0, 0.0], &[0.0, 0.0]), 0.0);
}

#[test]
fn voiceprint_files_round_trip_in_the_ts_format() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("destiny.json");
    let file = voiceprint(vec![vec![0.25, -0.5], vec![1.0, 0.000_001]]);
    std::fs::write(&path, file.to_json_line().expect("json")).expect("write");
    let text = std::fs::read_to_string(&path).expect("read");
    assert_eq!(
        text,
        "{\"version\":1,\"speaker\":\"destiny\",\"model\":\"3dspeaker-campplus-en-voxceleb-16k\",\"embeddings\":[[0.25,-0.5],[1,0.000001]],\"createdAt\":1,\"sources\":[]}\n"
    );
    assert_eq!(VoiceprintFile::load(Path::new(&path)).expect("loads"), file);
}
