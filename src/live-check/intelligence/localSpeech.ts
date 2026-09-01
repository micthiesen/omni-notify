import type { Effect as EffectType } from "effect/Effect";
import { access, readFile } from "node:fs/promises";
import { join } from "node:path";
import sherpa from "sherpa-onnx-node";
import { Data, Effect, Schema } from "effect";

const { OfflineRecognizer, SpeakerEmbeddingExtractor, Vad } = sherpa;

const SAMPLE_RATE = 16_000;
const SPEAKER_WINDOW_SECONDS = 4;
const SPEAKER_STRIDE_SECONDS = 2;

const EmbeddingSchema = Schema.Array(Schema.Finite).check(Schema.isMinLength(1));

export const VoiceprintFileSchema = Schema.Struct({
  version: Schema.Literal(1),
  speaker: Schema.Literal("destiny"),
  model: Schema.String,
  embeddings: Schema.Array(EmbeddingSchema).check(Schema.isMinLength(2)),
  createdAt: Schema.Number.check(Schema.isGreaterThanOrEqualTo(0)),
  sources: Schema.Array(Schema.String),
});

export type VoiceprintFile = typeof VoiceprintFileSchema.Type;

export interface SpeakerMatch {
  confidence: number;
  matchedWindows: number;
  checkedWindows: number;
}

export class SpeechRecognitionError extends Data.TaggedError("SpeechRecognitionError")<{
  readonly operation: string;
  readonly cause: unknown;
}> {}

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

function requireFilesEffect(
  paths: Record<string, string>,
): EffectType<void, SpeechRecognitionError> {
  return Effect.forEach(
    Object.values(paths),
    (path) =>
      Effect.tryPromise({
        try: () => access(path),
        catch: (cause) =>
          new SpeechRecognitionError({ operation: `access ${path}`, cause }),
      }).pipe(Effect.as(path)),
    { concurrency: "unbounded" },
  ).pipe(
    Effect.asVoid,
    Effect.mapError(
      (error) =>
        new SpeechRecognitionError({
          operation: "validate livestream speech model files",
          cause: error,
        }),
    ),
  );
}

function loadVoiceprintEffect(
  path: string,
): EffectType<VoiceprintFile, SpeechRecognitionError> {
  return Effect.tryPromise({
    try: () => readFile(path, "utf8"),
    catch: (cause) =>
      new SpeechRecognitionError({ operation: `read voiceprint ${path}`, cause }),
  }).pipe(
    Effect.flatMap((contents) =>
      Effect.try({
        try: () => JSON.parse(contents) as unknown,
        catch: (cause) =>
          new SpeechRecognitionError({ operation: `parse voiceprint ${path}`, cause }),
      }),
    ),
    Effect.flatMap((value) =>
      Schema.decodeUnknownEffect(VoiceprintFileSchema)(value).pipe(
        Effect.mapError(
          (cause) =>
            new SpeechRecognitionError({
              operation: `decode voiceprint ${path}`,
              cause,
            }),
        ),
      ),
    ),
  );
}

export class LocalSpeechRuntime {
  private readonly speakerExtractor: InstanceType<typeof SpeakerEmbeddingExtractor>;
  private readonly recognizer: InstanceType<typeof OfflineRecognizer>;
  private readonly voiceprint: VoiceprintFile | null;
  private readonly paths: ReturnType<typeof modelFiles>;

  private constructor(
    paths: ReturnType<typeof modelFiles>,
    voiceprint: VoiceprintFile | null,
    speakerExtractor: InstanceType<typeof SpeakerEmbeddingExtractor>,
    recognizer: InstanceType<typeof OfflineRecognizer>,
    private readonly speakerThreshold: number,
  ) {
    this.paths = paths;
    this.voiceprint = voiceprint;
    this.speakerExtractor = speakerExtractor;
    this.recognizer = recognizer;
  }

  public static createEffect(
    modelDir: string,
    voiceprintPath?: string,
    speakerThreshold = 0.62,
  ): EffectType<LocalSpeechRuntime, SpeechRecognitionError> {
    return Effect.gen(function* () {
      const paths = modelFiles(modelDir);
      yield* requireFilesEffect(paths);
      const voiceprint = voiceprintPath
        ? yield* loadVoiceprintEffect(voiceprintPath)
        : null;
      const native = yield* Effect.try({
        try: () => ({
          speakerExtractor: new SpeakerEmbeddingExtractor({
            model: paths.speaker,
            numThreads: 1,
            provider: "cpu",
            debug: 0,
          }),
          recognizer: new OfflineRecognizer({
            featConfig: { sampleRate: SAMPLE_RATE, featureDim: 80 },
            modelConfig: {
              transducer: {
                encoder: paths.encoder,
                decoder: paths.decoder,
                joiner: paths.joiner,
              },
              tokens: paths.tokens,
              numThreads: 3,
              provider: "cpu",
              debug: 0,
              modelType: "nemo_transducer",
            },
          }),
        }),
        catch: (cause) =>
          new SpeechRecognitionError({
            operation: "initialize livestream speech models",
            cause,
          }),
      });
      return new LocalSpeechRuntime(
        paths,
        voiceprint,
        native.speakerExtractor,
        native.recognizer,
        speakerThreshold,
      );
    });
  }

  public get hasVoiceprint(): boolean {
    return this.voiceprint !== null;
  }

  public extractSpeechEffect(
    samples: Float32Array,
  ): EffectType<Float32Array[], SpeechRecognitionError> {
    return Effect.try({
      try: () => {
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
      },
      catch: (cause) =>
        new SpeechRecognitionError({ operation: "extract livestream speech", cause }),
    });
  }

  public computeEmbeddingEffect(
    samples: Float32Array,
  ): EffectType<Float32Array, SpeechRecognitionError> {
    return Effect.try({
      try: () => {
        const stream = this.speakerExtractor.createStream();
        stream.acceptWaveform({ sampleRate: SAMPLE_RATE, samples });
        return stream;
      },
      catch: (cause) =>
        new SpeechRecognitionError({ operation: "prepare speaker embedding", cause }),
    }).pipe(
      Effect.flatMap((stream) =>
        this.speakerExtractor.isReady(stream)
          ? Effect.try({
              try: () => this.speakerExtractor.compute(stream, false),
              catch: (cause) =>
                new SpeechRecognitionError({
                  operation: "compute speaker embedding",
                  cause,
                }),
            })
          : Effect.fail(
              new SpeechRecognitionError({
                operation: "compute speaker embedding",
                cause: new Error("Speaker sample is too short for an embedding"),
              }),
            ),
      ),
    );
  }

  public detectDestinyEffect(
    samples: Float32Array,
  ): EffectType<SpeakerMatch, SpeechRecognitionError> {
    const voiceprint = this.voiceprint;
    if (!voiceprint)
      return Effect.succeed({ confidence: 0, matchedWindows: 0, checkedWindows: 0 });
    return Effect.gen({ self: this }, function* () {
      const speech = yield* this.extractSpeechEffect(samples);
      const windowSamples = SPEAKER_WINDOW_SECONDS * SAMPLE_RATE;
      const strideSamples = SPEAKER_STRIDE_SECONDS * SAMPLE_RATE;
      const windows: Float32Array[] = [];
      for (const segment of speech) {
        if (segment.length < windowSamples) continue;
        for (
          let offset = 0;
          offset + windowSamples <= segment.length;
          offset += strideSamples
        ) {
          windows.push(segment.subarray(offset, offset + windowSamples));
        }
      }
      const scores = yield* Effect.forEach(windows, (window) =>
        this.computeEmbeddingEffect(window).pipe(
          Effect.map((embedding) =>
            Math.max(
              ...voiceprint.embeddings.map((reference) =>
                cosineSimilarity(embedding, reference),
              ),
            ),
          ),
        ),
      );
      scores.sort((a, b) => b - a);
      const matchedWindows = scores.filter(
        (score) => score >= this.speakerThreshold,
      ).length;
      return {
        confidence: scores[0] ?? 0,
        matchedWindows,
        checkedWindows: scores.length,
      };
    });
  }

  public transcribeEffect(
    samples: Float32Array,
  ): EffectType<string, SpeechRecognitionError> {
    return Effect.tryPromise({
      try: () => {
        const stream = this.recognizer.createStream();
        stream.acceptWaveform({ sampleRate: SAMPLE_RATE, samples });
        return this.recognizer.decodeAsync(stream);
      },
      catch: (cause) =>
        new SpeechRecognitionError({ operation: "transcribe livestream audio", cause }),
    }).pipe(Effect.map((result) => result.text.trim()));
  }
}
