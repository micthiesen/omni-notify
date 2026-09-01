import { mkdir, writeFile } from "node:fs/promises";
import { dirname } from "node:path";
import { Data, Effect } from "effect";
import { runPromise } from "../effect/interop.js";
import { LivestreamAudioCapture } from "../live-check/intelligence/audio.js";
import {
  cosineSimilarity,
  LocalSpeechRuntime,
  type VoiceprintFile,
} from "../live-check/intelligence/localSpeech.js";

const SAMPLE_RATE = 16_000;
const WINDOW_SAMPLES = 4 * SAMPLE_RATE;

type Source = { url: string; seek: number };

function parseArgs(args: string[]): {
  output: string;
  modelDir: string;
  sources: Source[];
} {
  let output = "/data/livestream-intelligence/destiny.json";
  let modelDir =
    process.env.LIVESTREAM_MODEL_DIR ?? "/app/assets/livestream-intelligence/models";
  const sources: Source[] = [];
  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    if (arg === "--output") output = args[++index] ?? output;
    else if (arg === "--model-dir") modelDir = args[++index] ?? modelDir;
    else if (arg === "--source") {
      const url = args[++index];
      if (!url) throw new Error("--source requires a URL");
      sources.push({ url, seek: 0 });
    } else if (arg === "--seek") {
      const source = sources.at(-1);
      if (!source) throw new Error("--seek must follow a --source");
      source.seek = Number(args[++index]);
      if (!Number.isFinite(source.seek) || source.seek < 0) {
        throw new Error("--seek must be a non-negative number");
      }
    } else {
      throw new Error(`Unknown argument: ${arg}`);
    }
  }
  if (sources.length < 2) throw new Error("At least two --source clips are required");
  return { output, modelDir, sources };
}

type SourcedEmbedding = { embedding: Float32Array; sourceIndex: number };

class EnrollmentError extends Data.TaggedError("EnrollmentError")<{
  operation: string;
  cause: unknown;
}> {}

function selectCrossSourceCluster(embeddings: SourcedEmbedding[]): Float32Array[] {
  const CLUSTER_SIMILARITY = 0.5;
  const ranked = embeddings
    .map((candidate) => ({
      ...candidate,
      sourceCount: new Set(
        embeddings
          .filter(
            (item) =>
              cosineSimilarity(candidate.embedding, item.embedding) >=
              CLUSTER_SIMILARITY,
          )
          .map((item) => item.sourceIndex),
      ).size,
      centrality:
        embeddings.reduce(
          (sum, item) => sum + cosineSimilarity(candidate.embedding, item.embedding),
          0,
        ) / embeddings.length,
    }))
    .sort((a, b) => b.sourceCount - a.sourceCount || b.centrality - a.centrality);
  const center = ranked[0]?.embedding;
  if (!center || (ranked[0]?.sourceCount ?? 0) < 2) return [];
  return ranked
    .filter((item) => cosineSimilarity(center, item.embedding) >= CLUSTER_SIMILARITY)
    .slice(0, 16)
    .map((item) => item.embedding);
}

function mainEffect(): Effect.Effect<void, EnrollmentError> {
  return Effect.gen(function* () {
    const { output, modelDir, sources } = yield* Effect.try({
      try: () => parseArgs(process.argv.slice(2)),
      catch: (cause) => new EnrollmentError({ operation: "parse arguments", cause }),
    });
    const speech = yield* LocalSpeechRuntime.createEffect(modelDir).pipe(
      Effect.mapError(
        (cause) =>
          new EnrollmentError({ operation: "initialize speech runtime", cause }),
      ),
    );
    const capture = new LivestreamAudioCapture();
    const embeddings: SourcedEmbedding[] = [];
    for (const [sourceIndex, source] of sources.entries()) {
      const audio = yield* capture
        .captureEffect(source.url, 60, source.seek)
        .pipe(
          Effect.mapError(
            (cause) => new EnrollmentError({ operation: "capture audio", cause }),
          ),
        );
      const segments = yield* speech
        .extractSpeechEffect(audio.samples)
        .pipe(
          Effect.mapError(
            (cause) => new EnrollmentError({ operation: "extract speech", cause }),
          ),
        );
      let sourceEmbeddings = 0;
      for (const segment of segments) {
        for (
          let offset = 0;
          offset + WINDOW_SAMPLES <= segment.length;
          offset += WINDOW_SAMPLES
        ) {
          const embedding = yield* speech
            .computeEmbeddingEffect(segment.subarray(offset, offset + WINDOW_SAMPLES))
            .pipe(
              Effect.mapError(
                (cause) =>
                  new EnrollmentError({ operation: "compute embedding", cause }),
              ),
            );
          embeddings.push({
            embedding,
            sourceIndex,
          });
          sourceEmbeddings += 1;
        }
      }
      process.stdout.write(
        `Captured ${audio.durationSeconds.toFixed(1)}s from ${source.url}, ${sourceEmbeddings} speech windows\n`,
      );
    }
    const selected = selectCrossSourceCluster(embeddings);
    if (selected.length < 6) {
      return yield* Effect.fail(
        new EnrollmentError({
          operation: "select enrollment windows",
          cause: new Error(
            `Only ${selected.length} consistent speech windows found; need at least 6`,
          ),
        }),
      );
    }
    const voiceprint: VoiceprintFile = {
      version: 1,
      speaker: "destiny",
      model: "3dspeaker-campplus-en-voxceleb-16k",
      embeddings: selected.map((embedding) => Array.from(embedding)),
      createdAt: Date.now(),
      sources: sources.map((source) => `${source.url}#t=${source.seek}`),
    };
    yield* Effect.tryPromise({
      try: () => mkdir(dirname(output), { recursive: true }),
      catch: (cause) =>
        new EnrollmentError({ operation: "create output directory", cause }),
    });
    yield* Effect.tryPromise({
      try: () => writeFile(output, `${JSON.stringify(voiceprint)}\n`, { mode: 0o600 }),
      catch: (cause) => new EnrollmentError({ operation: "write voiceprint", cause }),
    });
    yield* Effect.sync(() =>
      process.stdout.write(
        `Wrote ${selected.length} enrollment embeddings to ${output}\n`,
      ),
    );
  });
}

await runPromise(mainEffect());
