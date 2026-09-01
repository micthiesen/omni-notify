import { LivestreamAudioCapture } from "../live-check/intelligence/audio.js";
import { LocalSpeechRuntime } from "../live-check/intelligence/localSpeech.js";
import { Data, Effect } from "effect";
import { runPromise } from "../effect/interop.js";

class DoctorError extends Data.TaggedError("DoctorError")<{
  operation: string;
  cause: unknown;
}> {}

function valueAfter(args: string[], flag: string): string | undefined {
  const index = args.indexOf(flag);
  return index >= 0 ? args[index + 1] : undefined;
}

const program = Effect.gen(function* () {
  const args = process.argv.slice(2);
  const url = valueAfter(args, "--url");
  if (!url) {
    return yield* Effect.fail(
      new DoctorError({
        operation: "parse arguments",
        cause: new Error("Usage: --url URL [--seek SECONDS] [--duration SECONDS]"),
      }),
    );
  }
  const duration = Number(valueAfter(args, "--duration") ?? 30);
  const seek = Number(valueAfter(args, "--seek") ?? 0);
  const modelDir =
    valueAfter(args, "--model-dir") ??
    process.env.LIVESTREAM_MODEL_DIR ??
    "/app/assets/livestream-intelligence/models";
  const voiceprint =
    valueAfter(args, "--voiceprint") ?? process.env.LIVESTREAM_DESTINY_VOICEPRINT_PATH;
  const startedAt = performance.now();
  const capture = new LivestreamAudioCapture();
  const audio = yield* capture
    .captureEffect(url, duration, seek)
    .pipe(
      Effect.mapError(
        (cause) => new DoctorError({ operation: "capture audio", cause }),
      ),
    );
  const capturedAt = performance.now();
  const speech = yield* LocalSpeechRuntime.createEffect(modelDir, voiceprint).pipe(
    Effect.mapError(
      (cause) => new DoctorError({ operation: "initialize speech runtime", cause }),
    ),
  );
  const transcript = yield* speech
    .transcribeEffect(audio.samples)
    .pipe(
      Effect.mapError(
        (cause) => new DoctorError({ operation: "transcribe audio", cause }),
      ),
    );
  const transcribedAt = performance.now();
  const match = yield* speech
    .detectDestinyEffect(audio.samples)
    .pipe(
      Effect.mapError(
        (cause) => new DoctorError({ operation: "detect speaker", cause }),
      ),
    );
  const finishedAt = performance.now();
  yield* Effect.sync(() =>
    process.stdout.write(
      `${JSON.stringify(
        {
          audioSeconds: audio.durationSeconds,
          captureSeconds: (capturedAt - startedAt) / 1000,
          transcriptionSeconds: (transcribedAt - capturedAt) / 1000,
          transcriptionRealtimeFactor:
            (transcribedAt - capturedAt) / 1000 / audio.durationSeconds,
          speakerSeconds: (finishedAt - transcribedAt) / 1000,
          match,
          transcript,
        },
        null,
        2,
      )}\n`,
    ),
  );
});

await runPromise(program);
