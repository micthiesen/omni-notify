import { setTimeout as sleep } from "node:timers/promises";
import type { Logger } from "@micthiesen/mitools/logging";
import got, { HTTPError } from "got";
import config from "../../../utils/config.js";
import { getVoice, type Voice } from "../voices.js";
import type { AuthorGender, TtsProvider } from "./types.js";

/** ElevenLabs v3 — "Natural" stability, fixed seed for reproducibility. */
const MODEL = "eleven_v3";
const ENDPOINT = "https://api.elevenlabs.io/v1/text-to-speech";
const OUTPUT_FORMAT = "mp3_44100_128";
const SEED = 4242;

export class ElevenLabsProvider implements TtsProvider {
  public readonly providerName = "ElevenLabs";
  public readonly modelId = MODEL;
  public readonly needsDenoise = false;
  public readonly verifyChunkLength = false;
  public readonly voiceName: string;
  private readonly voice: Voice;

  constructor(authorGender: AuthorGender) {
    this.voice = getVoice(authorGender);
    this.voiceName = this.voice.name;
  }

  public async synthesizeChunk(text: string, logger: Logger): Promise<Buffer> {
    const apiKey = config.ELEVENLABS_API_KEY;
    if (!apiKey) throw new Error("ELEVENLABS_API_KEY is not set");

    const maxAttempts = 3;
    for (let attempt = 1; ; attempt++) {
      try {
        const bytes = await got
          .post(`${ENDPOINT}/${this.voice.id}?output_format=${OUTPUT_FORMAT}`, {
            headers: { "xi-api-key": apiKey },
            json: {
              text,
              model_id: MODEL,
              seed: SEED,
              voice_settings: { stability: 0.5, use_speaker_boost: true },
            },
            timeout: { request: 5 * 60 * 1000 },
          })
          .buffer();
        return Buffer.from(bytes);
      } catch (error) {
        const status = error instanceof HTTPError ? error.response.statusCode : 0;
        const transient = status === 429 || status >= 500;
        if (!transient || attempt >= maxAttempts) throw error;
        const backoffMs = 2000 * 2 ** (attempt - 1);
        logger.warn(
          `ElevenLabs chunk failed (${status}); retry ${attempt}/${maxAttempts - 1} in ${backoffMs}ms`,
        );
        await sleep(backoffMs);
      }
    }
  }
}
