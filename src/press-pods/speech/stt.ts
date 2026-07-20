import type { Logger } from "@micthiesen/mitools/logging";
import { currentCostFeature, recordCostEventSafely } from "../../costs/persistence.js";
import config from "../../utils/config.js";

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

export interface SttClient {
  readonly modelId: string;
  /** Transcribe MP3 bytes to plain text. Throws on failure (caller degrades). */
  transcribe(mp3: Buffer, logger: Logger): Promise<string>;
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
    async transcribe(mp3: Buffer, logger: Logger): Promise<string> {
      for (let attempt = 1; ; attempt++) {
        try {
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
          const res = await fetch(`${baseUrl}/v1/audio/transcriptions`, {
            method: "POST",
            body: form,
            headers,
            signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS),
          });
          if (!res.ok) {
            const status = res.status;
            const err = new Error(`STT ${status}: ${(await res.text()).slice(0, 200)}`);
            if (!isTransient(status) || attempt >= MAX_ATTEMPTS) throw err;
            logger.debug(`${err.message}; retry ${attempt}/${MAX_ATTEMPTS - 1}`);
            continue;
          }
          const body = (await res.json()) as { text?: string };
          const selfHosted =
            config.PRESSPODS_STT_URL === undefined ||
            config.PRESSPODS_STT_URL === config.PRESSPODS_TTS_URL;
          recordCostEventSafely({
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
        } catch (error) {
          // A thrown 4xx (above) is already terminal; only network/abort errors
          // reach here as "transient". Give up after MAX_ATTEMPTS either way.
          if (error instanceof Error && error.message.startsWith("STT ")) throw error;
          if (attempt >= MAX_ATTEMPTS) throw error;
          logger.debug(
            `STT request failed (${(error as Error).message}); retry ${attempt}/${MAX_ATTEMPTS - 1}`,
          );
        }
      }
    },
  };
}
