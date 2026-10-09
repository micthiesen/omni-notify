//! Local speech: VAD, speaker embeddings, Destiny voiceprint matching and
//! transcription (`localSpeech.ts`).
//!
//! [`LocalSpeechRuntime`] holds the TS decision logic (windowing, scoring,
//! thresholds) over a [`SpeechBackend`]; [`crate::sherpa::SherpaBackend`] is the
//! native backend. The service talks to a [`SpeechEngine`], which runs the
//! blocking native work on tokio's blocking pool.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};

use crate::audio::SAMPLE_RATE;

/// Native model and default speaker threshold, identical to the Docker image layout.
pub const DEFAULT_SPEAKER_THRESHOLD: f64 = 0.62;
pub const SPEAKER_WINDOW_SECONDS: usize = 4;
pub const SPEAKER_STRIDE_SECONDS: usize = 2;
pub const TRANSCRIPTION_MODEL: &str = "parakeet-tdt-0.6b-v3-int8";
pub const VOICEPRINT_MODEL: &str = "3dspeaker-campplus-en-voxceleb-16k";

/// A failure inside the local speech pipeline.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{operation}: {cause}")]
pub struct SpeechRecognitionError {
    pub operation: String,
    pub cause: String,
}

impl SpeechRecognitionError {
    pub fn new(operation: impl Into<String>, cause: impl std::fmt::Display) -> Self {
        Self {
            operation: operation.into(),
            cause: cause.to_string(),
        }
    }
}

/// Best window score and how many windows reached the threshold.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SpeakerMatch {
    pub confidence: f64,
    pub matched_windows: u32,
    pub checked_windows: u32,
}

/// The six model paths under `LIVESTREAM_MODEL_DIR`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelFiles {
    pub vad: PathBuf,
    pub speaker: PathBuf,
    pub encoder: PathBuf,
    pub decoder: PathBuf,
    pub joiner: PathBuf,
    pub tokens: PathBuf,
}

impl ModelFiles {
    pub fn in_dir(model_dir: &Path) -> Self {
        let parakeet = model_dir.join("sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8");
        Self {
            vad: model_dir.join("silero_vad.int8.onnx"),
            speaker: model_dir.join("3dspeaker_speech_campplus_sv_en_voxceleb_16k.onnx"),
            encoder: parakeet.join("encoder.int8.onnx"),
            decoder: parakeet.join("decoder.int8.onnx"),
            joiner: parakeet.join("joiner.int8.onnx"),
            tokens: parakeet.join("tokens.txt"),
        }
    }

    pub fn all(&self) -> [&Path; 6] {
        [
            &self.vad,
            &self.speaker,
            &self.encoder,
            &self.decoder,
            &self.joiner,
            &self.tokens,
        ]
    }

    /// Every file must exist and be structurally sound before native
    /// initialization, which aborts the process on a bad file
    /// ([`crate::model_check`]).
    pub fn require(&self) -> Result<(), SpeechRecognitionError> {
        for path in self.all() {
            if let Err(cause) = std::fs::metadata(path) {
                return Err(SpeechRecognitionError::new(
                    "validate livestream speech model files",
                    SpeechRecognitionError::new(format!("access {}", path.display()), cause),
                ));
            }
            if let Err(cause) = crate::model_check::check_model_file(path) {
                return Err(SpeechRecognitionError::new(
                    "validate livestream speech model files",
                    SpeechRecognitionError::new(format!("check {}", path.display()), cause),
                ));
            }
        }
        Ok(())
    }
}

/// Voiceprint file format v1 (`LIVESTREAM_DESTINY_VOICEPRINT_PATH`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceprintFile {
    pub version: f64,
    pub speaker: String,
    pub model: String,
    pub embeddings: Vec<Vec<f64>>,
    pub created_at: f64,
    pub sources: Vec<String>,
}

impl VoiceprintFile {
    /// `VoiceprintFileSchema`: version 1, speaker "destiny", at least two
    /// non-empty finite embeddings, non-negative `createdAt`.
    pub fn validate(&self) -> Result<(), String> {
        #[allow(clippy::float_cmp)]
        if self.version != 1.0 {
            return Err(format!("version must be 1, got {}", self.version));
        }
        if self.speaker != "destiny" {
            return Err(format!(
                "speaker must be \"destiny\", got {:?}",
                self.speaker
            ));
        }
        if self.embeddings.len() < 2 {
            return Err(format!(
                "embeddings needs at least 2 items, got {}",
                self.embeddings.len()
            ));
        }
        for (index, embedding) in self.embeddings.iter().enumerate() {
            if embedding.is_empty() {
                return Err(format!("embeddings[{index}] is empty"));
            }
            if embedding.iter().any(|v| !v.is_finite()) {
                return Err(format!("embeddings[{index}] has a non-finite value"));
            }
        }
        if self.created_at.is_nan() || self.created_at < 0.0 {
            return Err(format!("createdAt must be >= 0, got {}", self.created_at));
        }
        Ok(())
    }

    /// Reads, parses and validates a voiceprint.
    pub fn load(path: &Path) -> Result<Self, SpeechRecognitionError> {
        let contents = std::fs::read_to_string(path).map_err(|cause| {
            SpeechRecognitionError::new(format!("read voiceprint {}", path.display()), cause)
        })?;
        let value: serde_json::Value = serde_json::from_str(&contents).map_err(|cause| {
            SpeechRecognitionError::new(format!("parse voiceprint {}", path.display()), cause)
        })?;
        let decode = |cause: String| {
            SpeechRecognitionError::new(format!("decode voiceprint {}", path.display()), cause)
        };
        let file: VoiceprintFile =
            serde_json::from_value(value).map_err(|e| decode(e.to_string()))?;
        file.validate().map_err(decode)?;
        Ok(file)
    }

    /// The file as TS writes it: `JSON.stringify(voiceprint)` plus a newline.
    pub fn to_json_line(&self) -> Result<String, serde_json::Error> {
        let value = serde_json::to_value(self)?;
        Ok(format!("{}\n", omni_core::js::json_stringify(&value)))
    }
}

/// Cosine similarity in double precision; `-1` for mismatched or empty vectors.
pub fn cosine_similarity(a: &[f32], b: &[f64]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return -1.0;
    }
    let (mut dot, mut norm_a, mut norm_b) = (0.0_f64, 0.0_f64, 0.0_f64);
    for (av, bv) in a.iter().zip(b) {
        let av = f64::from(*av);
        dot += av * bv;
        norm_a += av * av;
        norm_b += bv * bv;
    }
    dot / f64::EPSILON.max(norm_a.sqrt() * norm_b.sqrt())
}

/// The native operations the runtime needs. All calls block.
pub trait SpeechBackend: Send + Sync + 'static {
    /// VAD speech segments of at least one second.
    fn speech_segments(&self, samples: &[f32]) -> Result<Vec<Vec<f32>>, SpeechRecognitionError>;
    /// One speaker embedding.
    fn embedding(&self, samples: &[f32]) -> Result<Vec<f32>, SpeechRecognitionError>;
    /// Raw transcript text.
    fn transcribe(&self, samples: &[f32]) -> Result<String, SpeechRecognitionError>;
}

/// `LocalSpeechRuntime`: voiceprint matching and transcription over a backend.
pub struct LocalSpeechRuntime<B> {
    backend: B,
    voiceprint: Option<VoiceprintFile>,
    speaker_threshold: f64,
}

impl<B: SpeechBackend> LocalSpeechRuntime<B> {
    pub fn with_backend(backend: B, voiceprint: Option<VoiceprintFile>, threshold: f64) -> Self {
        Self {
            backend,
            voiceprint,
            speaker_threshold: threshold,
        }
    }

    /// Validates the model files, then loads the voiceprint, then initializes
    /// the backend: a bad voiceprint fails before any native initialization.
    pub fn create(
        model_dir: &Path,
        voiceprint_path: Option<&Path>,
        speaker_threshold: f64,
        init: impl FnOnce(&ModelFiles) -> Result<B, SpeechRecognitionError>,
    ) -> Result<Self, SpeechRecognitionError> {
        let files = ModelFiles::in_dir(model_dir);
        files.require()?;
        let voiceprint = voiceprint_path.map(VoiceprintFile::load).transpose()?;
        let backend = init(&files)?;
        Ok(Self::with_backend(backend, voiceprint, speaker_threshold))
    }

    pub fn has_voiceprint(&self) -> bool {
        self.voiceprint.is_some()
    }

    pub fn extract_speech(&self, samples: &[f32]) -> Result<Vec<Vec<f32>>, SpeechRecognitionError> {
        self.backend.speech_segments(samples)
    }

    pub fn compute_embedding(&self, samples: &[f32]) -> Result<Vec<f32>, SpeechRecognitionError> {
        self.backend.embedding(samples)
    }

    /// Scores 4 s windows (2 s stride) of each speech segment against every
    /// enrolled embedding; a window matches at `>= speaker_threshold`.
    pub fn detect_destiny(&self, samples: &[f32]) -> Result<SpeakerMatch, SpeechRecognitionError> {
        let Some(voiceprint) = &self.voiceprint else {
            return Ok(SpeakerMatch::default());
        };
        let speech = self.extract_speech(samples)?;
        let window = SPEAKER_WINDOW_SECONDS * SAMPLE_RATE as usize;
        let stride = SPEAKER_STRIDE_SECONDS * SAMPLE_RATE as usize;
        let mut scores = Vec::new();
        for segment in &speech {
            let mut offset = 0;
            while offset + window <= segment.len() {
                let embedding = self.compute_embedding(&segment[offset..offset + window])?;
                let best = voiceprint
                    .embeddings
                    .iter()
                    .map(|reference| cosine_similarity(&embedding, reference))
                    .fold(f64::NEG_INFINITY, crate::js_math::js_max);
                scores.push(best);
                offset += stride;
            }
        }
        scores.sort_by(|a, b| b.total_cmp(a));
        let matched = scores
            .iter()
            .filter(|score| **score >= self.speaker_threshold)
            .count();
        Ok(SpeakerMatch {
            confidence: scores.first().copied().unwrap_or(0.0),
            matched_windows: u32::try_from(matched).unwrap_or(u32::MAX),
            checked_windows: u32::try_from(scores.len()).unwrap_or(u32::MAX),
        })
    }

    /// Trimmed transcript.
    pub fn transcribe(&self, samples: &[f32]) -> Result<String, SpeechRecognitionError> {
        Ok(crate::js_math::js_trim(&self.backend.transcribe(samples)?).to_owned())
    }
}

/// The service's speech seam.
pub trait SpeechEngine: Send + Sync + 'static {
    fn has_voiceprint(&self) -> bool;
    fn detect_destiny(
        &self,
        samples: Arc<Vec<f32>>,
    ) -> BoxFuture<'_, Result<SpeakerMatch, SpeechRecognitionError>>;
    fn transcribe(
        &self,
        samples: Arc<Vec<f32>>,
    ) -> BoxFuture<'_, Result<String, SpeechRecognitionError>>;
}

/// Runs a [`LocalSpeechRuntime`] on tokio's blocking pool.
pub struct BlockingSpeech<B>(pub Arc<LocalSpeechRuntime<B>>);

impl<B> Clone for BlockingSpeech<B> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

async fn blocking<T: Send + 'static>(
    operation: &'static str,
    f: impl FnOnce() -> Result<T, SpeechRecognitionError> + Send + 'static,
) -> Result<T, SpeechRecognitionError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| SpeechRecognitionError::new(operation, e))?
}

impl<B: SpeechBackend> SpeechEngine for BlockingSpeech<B> {
    fn has_voiceprint(&self) -> bool {
        self.0.has_voiceprint()
    }

    fn detect_destiny(
        &self,
        samples: Arc<Vec<f32>>,
    ) -> BoxFuture<'_, Result<SpeakerMatch, SpeechRecognitionError>> {
        let runtime = Arc::clone(&self.0);
        Box::pin(blocking("detect speaker", move || {
            runtime.detect_destiny(&samples)
        }))
    }

    fn transcribe(
        &self,
        samples: Arc<Vec<f32>>,
    ) -> BoxFuture<'_, Result<String, SpeechRecognitionError>> {
        let runtime = Arc::clone(&self.0);
        Box::pin(blocking("transcribe livestream audio", move || {
            runtime.transcribe(&samples)
        }))
    }
}
