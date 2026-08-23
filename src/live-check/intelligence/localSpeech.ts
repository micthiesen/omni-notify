import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import sherpa from "sherpa-onnx-node";

const { OfflineRecognizer, SpeakerEmbeddingExtractor, Vad } = sherpa;

const SAMPLE_RATE = 16_000;
const SPEAKER_WINDOW_SECONDS = 4;
const SPEAKER_STRIDE_SECONDS = 2;

export interface VoiceprintFile {
  version: 1;
  speaker: "destiny";
  model: string;
  embeddings: number[][];
  createdAt: number;
  sources: string[];
}

export interface SpeakerMatch {
  confidence: number;
  matchedWindows: number;
  checkedWindows: number;
}

export function cosineSimilarity(a: Float32Array, b: ArrayLike<number>): number {
  if (a.length !== b.length || a.length === 0) return -1;
  let dot = 0;
  let normA = 0;
  let normB = 0;
  for (let index = 0; index < a.length; index += 1) {
    const av = a[index] ?? 0;
    const bv = b[index] ?? 0;
    dot += av * bv;
    normA += av * av;
    normB += bv * bv;
  }
  return dot / Math.max(Number.EPSILON, Math.sqrt(normA) * Math.sqrt(normB));
}

function modelFiles(modelDir: string) {
  const parakeet = join(modelDir, "sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8");
  return {
    vad: join(modelDir, "silero_vad.int8.onnx"),
    speaker: join(modelDir, "3dspeaker_speech_campplus_sv_en_voxceleb_16k.onnx"),
    encoder: join(parakeet, "encoder.int8.onnx"),
    decoder: join(parakeet, "decoder.int8.onnx"),
    joiner: join(parakeet, "joiner.int8.onnx"),
    tokens: join(parakeet, "tokens.txt"),
  };
}

function requireFiles(paths: Record<string, string>): void {
  const missing = Object.values(paths).filter((path) => !existsSync(path));
  if (missing.length > 0) {
    throw new Error(`Livestream speech model files missing: ${missing.join(", ")}`);
  }
}

export class LocalSpeechRuntime {
  private readonly speakerExtractor: InstanceType<typeof SpeakerEmbeddingExtractor>;
  private readonly recognizer: InstanceType<typeof OfflineRecognizer>;
  private readonly voiceprint: VoiceprintFile | null;
  private readonly paths: ReturnType<typeof modelFiles>;

  public constructor(
    modelDir: string,
    voiceprintPath?: string,
    private readonly speakerThreshold = 0.62,
  ) {
    this.paths = modelFiles(modelDir);
    requireFiles(this.paths);
    this.speakerExtractor = new SpeakerEmbeddingExtractor({
      model: this.paths.speaker,
      numThreads: 1,
      provider: "cpu",
      debug: 0,
    });
    this.recognizer = new OfflineRecognizer({
      featConfig: { sampleRate: SAMPLE_RATE, featureDim: 80 },
      modelConfig: {
        transducer: {
          encoder: this.paths.encoder,
          decoder: this.paths.decoder,
          joiner: this.paths.joiner,
        },
        tokens: this.paths.tokens,
        numThreads: 3,
        provider: "cpu",
        debug: 0,
        modelType: "nemo_transducer",
      },
    });
    this.voiceprint = voiceprintPath
      ? (JSON.parse(readFileSync(voiceprintPath, "utf8")) as VoiceprintFile)
      : null;
    if (this.voiceprint && this.voiceprint.embeddings.length < 2) {
      throw new Error("Destiny voiceprint needs at least two enrollment embeddings");
    }
  }

  public get hasVoiceprint(): boolean {
    return this.voiceprint !== null;
  }

  public extractSpeech(samples: Float32Array): Float32Array[] {
    const vad = new Vad(
      {
        sileroVad: {
          model: this.paths.vad,
          threshold: 0.5,
          minSpeechDuration: 0.25,
          minSilenceDuration: 0.35,
          windowSize: 512,
          maxSpeechDuration: 20,
        },
        sampleRate: SAMPLE_RATE,
        numThreads: 1,
        provider: "cpu",
        debug: 0,
      },
      120,
    );
    for (let offset = 0; offset < samples.length; offset += 512) {
      vad.acceptWaveform(
        samples.subarray(offset, Math.min(samples.length, offset + 512)),
      );
    }
    vad.flush();
    const segments: Float32Array[] = [];
    while (!vad.isEmpty()) {
      const segment = vad.front(false);
      if (segment.samples.length >= SAMPLE_RATE) segments.push(segment.samples);
      vad.pop();
    }
    return segments;
  }

  public computeEmbedding(samples: Float32Array): Float32Array {
    const stream = this.speakerExtractor.createStream();
    stream.acceptWaveform({ sampleRate: SAMPLE_RATE, samples });
    if (!this.speakerExtractor.isReady(stream)) {
      throw new Error("Speaker sample is too short for an embedding");
    }
    return this.speakerExtractor.compute(stream, false);
  }

  public detectDestiny(samples: Float32Array): SpeakerMatch {
    if (!this.voiceprint)
      return { confidence: 0, matchedWindows: 0, checkedWindows: 0 };
    const speech = this.extractSpeech(samples);
    const windowSamples = SPEAKER_WINDOW_SECONDS * SAMPLE_RATE;
    const strideSamples = SPEAKER_STRIDE_SECONDS * SAMPLE_RATE;
    const scores: number[] = [];
    for (const segment of speech) {
      if (segment.length < windowSamples) continue;
      for (
        let offset = 0;
        offset + windowSamples <= segment.length;
        offset += strideSamples
      ) {
        const embedding = this.computeEmbedding(
          segment.subarray(offset, offset + windowSamples),
        );
        scores.push(
          Math.max(
            ...this.voiceprint.embeddings.map((reference) =>
              cosineSimilarity(embedding, reference),
            ),
          ),
        );
      }
    }
    scores.sort((a, b) => b - a);
    const matchedWindows = scores.filter(
      (score) => score >= this.speakerThreshold,
    ).length;
    return {
      confidence: scores[0] ?? 0,
      matchedWindows,
      checkedWindows: scores.length,
    };
  }

  public async transcribe(samples: Float32Array): Promise<string> {
    const stream = this.recognizer.createStream();
    stream.acceptWaveform({ sampleRate: SAMPLE_RATE, samples });
    const result = await this.recognizer.decodeAsync(stream);
    return result.text.trim();
  }
}
