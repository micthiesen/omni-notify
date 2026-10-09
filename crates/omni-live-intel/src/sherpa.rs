//! The sherpa-onnx backend (safe wrapper only): Silero VAD, 3D-Speaker CAM++
//! embeddings and the Parakeet TDT 0.6B v3 int8 transducer, all on CPU,
//! configured so embeddings match the enrolled voiceprints.

use std::path::Path;
use std::sync::Mutex;

use sherpa_onnx::{
    OfflineRecognizer, OfflineRecognizerConfig, OfflineTransducerModelConfig, SileroVadModelConfig,
    SpeakerEmbeddingExtractor, SpeakerEmbeddingExtractorConfig, VadModelConfig,
    VoiceActivityDetector,
};

use crate::audio::SAMPLE_RATE;
use crate::speech::{ModelFiles, SpeechBackend, SpeechRecognitionError};

const VAD_WINDOW: usize = 512;
const VAD_BUFFER_SECONDS: f32 = 120.0;

fn path_string(path: &Path) -> Option<String> {
    Some(path.to_string_lossy().into_owned())
}

#[allow(clippy::cast_possible_wrap)]
const SAMPLE_RATE_I32: i32 = SAMPLE_RATE as i32;

/// VAD and speaker embeddings: everything voice matching and enrollment need.
/// The extractor is created once; a VAD is created per call because it is
/// stateful.
pub struct SherpaSpeaker {
    vad_config: VadModelConfig,
    speaker: Mutex<SpeakerEmbeddingExtractor>,
}

fn init_error(what: &str) -> SpeechRecognitionError {
    SpeechRecognitionError::new(
        "initialize livestream speech models",
        format!("sherpa-onnx could not create the {what}"),
    )
}

impl SherpaSpeaker {
    pub fn new(vad: &Path, speaker: &Path) -> Result<Self, SpeechRecognitionError> {
        let extractor = SpeakerEmbeddingExtractor::create(&SpeakerEmbeddingExtractorConfig {
            model: path_string(speaker),
            num_threads: 1,
            debug: false,
            provider: Some("cpu".to_owned()),
        })
        .ok_or_else(|| init_error("speaker embedding extractor"))?;
        let vad_config = VadModelConfig {
            silero_vad: SileroVadModelConfig {
                model: path_string(vad),
                threshold: 0.5,
                min_silence_duration: 0.35,
                min_speech_duration: 0.25,
                #[allow(clippy::cast_possible_wrap, clippy::cast_possible_truncation)]
                window_size: VAD_WINDOW as i32,
                max_speech_duration: 20.0,
            },
            sample_rate: SAMPLE_RATE_I32,
            num_threads: 1,
            provider: Some("cpu".to_owned()),
            debug: false,
            ..VadModelConfig::default()
        };
        Ok(Self {
            vad_config,
            speaker: Mutex::new(extractor),
        })
    }

    /// VAD speech segments of at least one second.
    pub fn speech_segments(
        &self,
        samples: &[f32],
    ) -> Result<Vec<Vec<f32>>, SpeechRecognitionError> {
        let vad = VoiceActivityDetector::create(&self.vad_config, VAD_BUFFER_SECONDS).ok_or_else(
            || SpeechRecognitionError::new("extract livestream speech", "VAD creation failed"),
        )?;
        for chunk in samples.chunks(VAD_WINDOW) {
            vad.accept_waveform(chunk);
        }
        vad.flush();
        let mut segments = Vec::new();
        while let Some(segment) = vad.front() {
            if segment.samples().len() >= SAMPLE_RATE as usize {
                segments.push(segment.samples().to_vec());
            }
            vad.pop();
        }
        Ok(segments)
    }

    /// One speaker embedding.
    pub fn embedding(&self, samples: &[f32]) -> Result<Vec<f32>, SpeechRecognitionError> {
        let extractor = lock(&self.speaker);
        let stream = extractor.create_stream().ok_or_else(|| {
            SpeechRecognitionError::new("prepare speaker embedding", "stream creation failed")
        })?;
        // No input_finished, so embeddings stay comparable with the enrolled
        // voiceprints.
        stream.accept_waveform(SAMPLE_RATE_I32, samples);
        if !extractor.is_ready(&stream) {
            return Err(SpeechRecognitionError::new(
                "compute speaker embedding",
                "Speaker sample is too short for an embedding",
            ));
        }
        extractor.compute(&stream).ok_or_else(|| {
            SpeechRecognitionError::new("compute speaker embedding", "no embedding returned")
        })
    }
}

/// Native models: [`SherpaSpeaker`] plus the transducer recognizer.
pub struct SherpaBackend {
    speaker: SherpaSpeaker,
    recognizer: Mutex<OfflineRecognizer>,
}

impl SherpaBackend {
    pub fn new(files: &ModelFiles) -> Result<Self, SpeechRecognitionError> {
        let speaker = SherpaSpeaker::new(&files.vad, &files.speaker)?;
        let mut config = OfflineRecognizerConfig::default();
        config.model_config.transducer = OfflineTransducerModelConfig {
            encoder: path_string(&files.encoder),
            decoder: path_string(&files.decoder),
            joiner: path_string(&files.joiner),
        };
        config.model_config.tokens = path_string(&files.tokens);
        config.model_config.num_threads = 3;
        config.model_config.provider = Some("cpu".to_owned());
        config.model_config.debug = false;
        config.model_config.model_type = Some("nemo_transducer".to_owned());
        config.decoding_method = Some("greedy_search".to_owned());
        let recognizer =
            OfflineRecognizer::create(&config).ok_or_else(|| init_error("offline recognizer"))?;
        Ok(Self {
            speaker,
            recognizer: Mutex::new(recognizer),
        })
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl SpeechBackend for SherpaBackend {
    fn speech_segments(&self, samples: &[f32]) -> Result<Vec<Vec<f32>>, SpeechRecognitionError> {
        self.speaker.speech_segments(samples)
    }

    fn embedding(&self, samples: &[f32]) -> Result<Vec<f32>, SpeechRecognitionError> {
        self.speaker.embedding(samples)
    }

    fn transcribe(&self, samples: &[f32]) -> Result<String, SpeechRecognitionError> {
        let recognizer = lock(&self.recognizer);
        let stream = recognizer.create_stream();
        stream.accept_waveform(SAMPLE_RATE_I32, samples);
        recognizer.decode(&stream);
        stream.get_result().map(|r| r.text).ok_or_else(|| {
            SpeechRecognitionError::new("transcribe livestream audio", "no recognition result")
        })
    }
}
