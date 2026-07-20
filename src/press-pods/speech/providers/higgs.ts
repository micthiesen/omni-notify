import { setTimeout as sleep } from "node:timers/promises";
import type { Logger } from "@micthiesen/mitools/logging";
import got, { HTTPError, RequestError } from "got";
import config from "../../../utils/config.js";
import type { AuthorGender, TtsProvider } from "./types.js";

/** Higgs Audio v3 via a self-hosted mlx-audio server (OpenAI-shaped API). */
const DEFAULT_MODEL = "bosonai/higgs-audio-v3-tts-4b";
/** Higgs reads ~20 chars/s at 1.0; 0.9 lands nearer a natural narration pace. */
const SPEED = 0.9;
/** Output token cap. The server default (1200) truncates ~900-char chunks; a
 * complete chunk needs ~1700, so this leaves headroom while bounding how long
 * a runaway can generate before the length-verify in synthesize.ts rejects it. */
const MAX_TOKENS = 3000;
/** One request per chunk should finish in well under this; a runaway hits the
 * token cap first. Bounds a genuinely hung server. */
const REQUEST_TIMEOUT_MS = 4 * 60 * 1000;

interface RefVoice {
  refAudio: string;
  refText: string;
}

export class HiggsProvider implements TtsProvider {
  public readonly providerName = "Higgs";
  public readonly needsDenoise = true;
  public readonly verifyChunkLength = true;
  public readonly verifyChunkContent = true;
  public readonly modelId: string;
  public readonly voiceName: string;
  private readonly baseUrl: string;
  private readonly gender: "male" | "female";
  private readonly refVoice: RefVoice | undefined;

  constructor(authorGender: AuthorGender) {
    if (!config.PRESSPODS_TTS_URL) {
      throw new Error("PRESSPODS_TTS_URL is not set (required for the Higgs provider)");
    }
    this.baseUrl = config.PRESSPODS_TTS_URL;
    this.modelId = config.PRESSPODS_TTS_MODEL ?? DEFAULT_MODEL;
    this.gender = authorGender === "male" ? "male" : "female";
    // A reference clip pins one consistent voice across chunks (Higgs otherwise
    // picks a random speaker per request). Both fields are needed; the path
    // resolves on the mlx-audio host.
    this.refVoice =
      config.PRESSPODS_HIGGS_REF_AUDIO && config.PRESSPODS_HIGGS_REF_TEXT
        ? {
            refAudio: config.PRESSPODS_HIGGS_REF_AUDIO,
            refText: config.PRESSPODS_HIGGS_REF_TEXT,
          }
        : undefined;
    this.voiceName = this.refVoice ? "Higgs (cloned)" : `Higgs (${this.gender})`;
  }

  public async synthesizeChunk(text: string, logger: Logger): Promise<Buffer> {
    // A reference clip defines the voice, so gender is omitted when cloning.
    const voiceParams = this.refVoice
      ? { ref_audio: this.refVoice.refAudio, ref_text: this.refVoice.refText }
      : { gender: this.gender };
    // Only network blips retry here; content-quality retries (truncation /
    // runaway) are the length-verify's job in synthesize.ts, so keep this small
    // to avoid the two loops compounding into a long stall.
    const maxAttempts = 2;
    for (let attempt = 1; ; attempt++) {
      try {
        const bytes = await got
          .post(`${this.baseUrl}/v1/audio/speech`, {
            json: {
              model: this.modelId,
              input: text,
              ...voiceParams,
              speed: SPEED,
              max_tokens: MAX_TOKENS,
              response_format: "mp3",
            },
            timeout: { request: REQUEST_TIMEOUT_MS },
          })
          .buffer();
        return Buffer.from(bytes);
      } catch (error) {
        // Retry 5xx/429 and non-HTTP errors (connection resets, timeouts,
        // truncated streams) — but NOT 4xx: HTTPError extends RequestError, so
        // classify by whether there's an HTTP response first.
        const transient =
          error instanceof HTTPError
            ? error.response.statusCode === 429 || error.response.statusCode >= 500
            : error instanceof RequestError;
        if (!transient || attempt >= maxAttempts) throw error;
        const backoffMs = 2000 * 2 ** (attempt - 1);
        logger.warn(
          `Higgs request failed (${(error as Error).message}); retry ${attempt}/${maxAttempts - 1} in ${backoffMs}ms`,
        );
        await sleep(backoffMs);
      }
    }
  }
}
