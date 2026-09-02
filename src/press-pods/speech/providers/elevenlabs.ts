import type { NamedLogger as Logger } from "@micthiesen/mitools/logging";
import got from "got";
import { Duration, Effect, Schedule } from "effect";
import { readBufferResponseWithLimit } from "../../../effect/publicHttp.js";
import { isTransientHttpError } from "../../../effect/errors.js";
import config from "../../../utils/config.js";
import { PressPodsError, tryPromise } from "../../effect.js";
import { getVoice, type Voice } from "../voices.js";
import type { AuthorGender, TtsProvider } from "./types.js";

/** ElevenLabs v3 — "Natural" stability, fixed seed for reproducibility. */
const MODEL = "eleven_v3";
const ENDPOINT = "https://api.elevenlabs.io/v1/text-to-speech";
const OUTPUT_FORMAT = "mp3_44100_128";
const SEED = 4242;
const MAX_TTS_CHUNK_BYTES = 25 * 1024 * 1024;

export class ElevenLabsProvider implements TtsProvider {
  public readonly providerName = "ElevenLabs";
  public readonly modelId = MODEL;
  public readonly needsDenoise = false;
  public readonly verifyChunkLength = false;
  public readonly verifyChunkContent = false;
  public readonly voiceName: string;
  private readonly voice: Voice;

  constructor(authorGender: AuthorGender) {
    this.voice = getVoice(authorGender);
    this.voiceName = this.voice.name;
  }

  public synthesizeChunk(text: string, logger: Logger) {
    const apiKey = config.ELEVENLABS_API_KEY;
    if (!apiKey) {
      return Effect.fail(
        new PressPodsError({
          operation: "synthesize ElevenLabs chunk",
          cause: new Error("ELEVENLABS_API_KEY is not set"),
        }),
      );
    }
    const request = tryPromise("synthesize ElevenLabs chunk", (signal) =>
      readBufferResponseWithLimit(
        got.stream(`${ENDPOINT}/${this.voice.id}?output_format=${OUTPUT_FORMAT}`, {
          method: "POST",
          headers: { "xi-api-key": apiKey },
          json: {
            text,
            model_id: MODEL,
            seed: SEED,
            voice_settings: { stability: 0.5, use_speaker_boost: true },
          },
          timeout: { request: 5 * 60 * 1000 },
          signal,
        }),
        MAX_TTS_CHUNK_BYTES,
      ),
    );
    return request.pipe(
      Effect.tapError((error) =>
        logger.warn("ElevenLabs chunk request failed", { error }),
      ),
      Effect.retry({
        times: 2,
        schedule: Schedule.exponential(Duration.seconds(2)),
        while: isTransientHttpError,
      }),
    );
  }
}
