import type { NamedLogger as Logger } from "@micthiesen/mitools/logging";
import { Duration, Effect, Schedule, Schema } from "effect";
import { currentCostFeature, recordCostEventSafely } from "../../costs/persistence.js";
import { readFetchResponseTextWithLimit } from "../../effect/publicHttp.js";
import config from "../../utils/config.js";
import type { TaskServices } from "../../task-runs/registry.js";
import { InvalidPressPodsDataError, PressPodsError, tryPromise } from "../effect.js";

/**
 * Speech-to-text for content verification. Points at an OpenAI-compatible
 * `/v1/audio/transcriptions` endpoint — by default the same mlx-audio host that
 * serves Higgs TTS (the M5 already loads an ASR model), so verification is $0
 * and local. Only used to catch truncated/looping TTS output (see coverage.ts);
 * transcription accuracy only has to be good enough to count words.
 */

/** Fast, accurate, self-hostable; whisper-large-v3-turbo on this mlx build 500s. */
const DEFAULT_MODEL = "mlx-community/parakeet-tdt-0.6b-v3";
/** One short chunk transcribes in well under a second; bound a hung server. */
const REQUEST_TIMEOUT_MS = 60 * 1000;
const MAX_ATTEMPTS = 2;
const STT_ERROR_MAX_BYTES = 64 * 1024;
const STT_JSON_MAX_BYTES = 1024 * 1024;

export interface SttClient {
  readonly modelId: string;
  /** Transcribe MP3 bytes to plain text. Throws on failure (caller degrades). */
  transcribe(
    mp3: Buffer,
    logger: Logger,
  ): Effect.Effect<string, PressPodsError | InvalidPressPodsDataError, TaskServices>;
}

/** Retry network blips / 5xx / 429, but never a 4xx (bad request won't fix). */
function isTransient(status: number | undefined): boolean {
  return status === undefined || status === 429 || status >= 500;
}

/**
 * Build the STT client, or `null` when no endpoint is configured (verification
 * then degrades to the duration-band check). Resolution: an explicit
 * `PRESSPODS_STT_URL`, else the Higgs `PRESSPODS_TTS_URL` (same box).
 */
export function createSttClient(apiKey?: string): SttClient | null {
  const baseUrl = config.PRESSPODS_STT_URL ?? config.PRESSPODS_TTS_URL;
  if (!baseUrl) return null;
  const modelId = config.PRESSPODS_STT_MODEL ?? DEFAULT_MODEL;
  const headers = apiKey ? { authorization: `Bearer ${apiKey}` } : undefined;

  return {
    modelId,
    transcribe(mp3: Buffer, logger: Logger) {
      const request = Effect.gen(function* () {
        const form = new FormData();
        form.set("model", modelId);
        form.set("response_format", "json");
        // Copy into a fresh ArrayBuffer-backed view (Buffer's is typed as
        // ArrayBufferLike, which Blob's typings reject).
        form.set(
          "file",
          new Blob([Uint8Array.from(mp3)], { type: "audio/mpeg" }),
          "chunk.mp3",
        );
        const res = yield* tryPromise("transcribe PressPods chunk", (signal) =>
          fetch(`${baseUrl}/v1/audio/transcriptions`, {
            method: "POST",
            body: form,
            headers,
            signal,
          }),
        ).pipe(
          Effect.timeout(Duration.millis(REQUEST_TIMEOUT_MS)),
          Effect.mapError((cause) =>
            cause instanceof PressPodsError
              ? cause
              : new PressPodsError({
                  operation: "transcribe PressPods chunk",
                  cause,
                  retryable: true,
                }),
          ),
        );
        if (!res.ok) {
          const status = res.status;
          const body = yield* tryPromise("read STT error response", (signal) =>
            readFetchResponseTextWithLimit(res, STT_ERROR_MAX_BYTES, signal),
          );
          return yield* new PressPodsError({
            operation: "transcribe PressPods chunk",
            cause: Object.assign(new Error(`STT ${status}: ${body.slice(0, 200)}`), {
              statusCode: status,
            }),
            retryable: isTransient(status),
          });
        }
        const responseText = yield* tryPromise("read STT response", (signal) =>
          readFetchResponseTextWithLimit(res, STT_JSON_MAX_BYTES, signal),
        );
        const raw = yield* Effect.try({
          try: () => JSON.parse(responseText) as unknown,
          catch: (cause) =>
            new InvalidPressPodsDataError({
              operation: "parse STT response JSON",
              cause,
            }),
        });
        const body = yield* Schema.decodeUnknownEffect(
          Schema.Struct({ text: Schema.optional(Schema.String) }),
        )(raw).pipe(
          Effect.mapError(
            (cause) =>
              new InvalidPressPodsDataError({
                operation: "decode STT response",
                cause,
              }),
          ),
        );
        const selfHosted =
          config.PRESSPODS_STT_URL === undefined ||
          config.PRESSPODS_STT_URL === config.PRESSPODS_TTS_URL;
        yield* recordCostEventSafely({
          category: "transcription",
          feature: currentCostFeature("press-pods"),
          operation: "verify-audio",
          service: selfHosted ? "self-hosted" : "openai-compatible",
          model: modelId,
          costCents: selfHosted ? 0 : null,
          priceStatus: selfHosted ? "free" : "unknown",
          usage: { requests: 1 },
        });
        return (body.text ?? "").trim();
      });
      return request.pipe(
        Effect.tapError((error) =>
          logger.debug(`STT request failed (${error.message})`),
        ),
        Effect.retry({
          times: MAX_ATTEMPTS - 1,
          schedule: Schedule.exponential(Duration.seconds(1)),
          while: (error) =>
            error instanceof PressPodsError && error.retryable !== false,
        }),
      );
    },
  };
}
