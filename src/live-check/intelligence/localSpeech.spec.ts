import type { Effect as EffectType } from "effect/Effect";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "@effect/vitest";
import { Effect } from "effect";
import { vi } from "vitest";
import { LocalSpeechRuntime, SpeechRecognitionError } from "./localSpeech.js";

vi.mock("sherpa-onnx-node", () => ({
  default: {
    OfflineRecognizer: class {},
    SpeakerEmbeddingExtractor: class {},
    Vad: class {},
  },
}));

function filesystemEffect<A>(operation: string, run: () => Promise<A>) {
  return Effect.tryPromise({
    try: run,
    catch: (cause) => new SpeechRecognitionError({ operation, cause }),
  });
}

function withSpeechFixture<A, E>(
  voiceprintContents: string,
  use: (fixture: { modelDir: string; voiceprintPath: string }) => EffectType<A, E>,
): EffectType<A, E | SpeechRecognitionError> {
  return Effect.acquireUseRelease(
    filesystemEffect("create speech fixture", () =>
      mkdtemp(join(tmpdir(), "omni-local-speech-")),
    ),
    (modelDir) =>
      Effect.gen(function* () {
        const parakeet = join(modelDir, "sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8");
        yield* filesystemEffect("create speech model fixture", () =>
          mkdir(parakeet, { recursive: true }),
        );
        yield* Effect.forEach(
          [
            join(modelDir, "silero_vad.int8.onnx"),
            join(modelDir, "3dspeaker_speech_campplus_sv_en_voxceleb_16k.onnx"),
            join(parakeet, "encoder.int8.onnx"),
            join(parakeet, "decoder.int8.onnx"),
            join(parakeet, "joiner.int8.onnx"),
            join(parakeet, "tokens.txt"),
          ],
          (path) =>
            filesystemEffect("write speech model fixture", () => writeFile(path, "")),
          { concurrency: "unbounded" },
        );
        const voiceprintPath = join(modelDir, "destiny.json");
        yield* filesystemEffect("write voiceprint fixture", () =>
          writeFile(voiceprintPath, voiceprintContents),
        );
        return yield* use({ modelDir, voiceprintPath });
      }),
    (modelDir) =>
      filesystemEffect("remove speech fixture", () =>
        rm(modelDir, { recursive: true, force: true }),
      ).pipe(Effect.ignore),
  );
}

describe("LocalSpeechRuntime.createEffect", () => {
  it.effect("returns a typed failure for malformed voiceprint JSON", () =>
    withSpeechFixture("{not-json", ({ modelDir, voiceprintPath }) =>
      Effect.gen(function* () {
        const error = yield* Effect.flip(
          LocalSpeechRuntime.createEffect(modelDir, voiceprintPath),
        );
        expect(error).toBeInstanceOf(SpeechRecognitionError);
        expect(error.operation).toContain("parse voiceprint");
      }),
    ),
  );

  it.effect(
    "rejects a structurally invalid voiceprint before native initialization",
    () =>
      withSpeechFixture(
        JSON.stringify({
          version: 1,
          speaker: "destiny",
          model: "test",
          embeddings: [[0.1, 0.2]],
          createdAt: 1,
          sources: ["test"],
        }),
        ({ modelDir, voiceprintPath }) =>
          Effect.gen(function* () {
            const error = yield* Effect.flip(
              LocalSpeechRuntime.createEffect(modelDir, voiceprintPath),
            );
            expect(error).toBeInstanceOf(SpeechRecognitionError);
            expect(error.operation).toContain("decode voiceprint");
          }),
      ),
  );
});
