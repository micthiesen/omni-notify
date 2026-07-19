import fsAsync from "node:fs/promises";
import { setTimeout as sleep } from "node:timers/promises";
import type { Logger } from "@micthiesen/mitools/logging";
import got, { HTTPError } from "got";
import config from "../../utils/config.js";
import type { MetadataInfo } from "../agents/metadata.js";
import type CostCounter from "../costs.js";
import type { Chapter } from "../types.js";
import {
  assembleEpisode,
  cleanupWavs,
  makeSilenceWav,
  prepareChunk,
  probeDurationSeconds,
} from "./audioChain.js";
import { chunkText, splitSections } from "./textChunking.js";
import { getVoice, type Voice } from "./voices.js";

/** ElevenLabs v3 — highest-expressiveness model; "Natural" stability mode. */
export const TTS_MODEL = "eleven_v3";
const TTS_ENDPOINT = "https://api.elevenlabs.io/v1/text-to-speech";
const OUTPUT_FORMAT = "mp3_44100_128";
/** Deterministic sampling so a re-synthesized episode sounds identical. */
const SEED = 4242;

/** v3 sweet spot is 500-800 chars; keep headroom under the 5k cap. */
const CHUNK_TARGET = 900;
const CHUNK_MAX = 1500;
/** Gaps between chunks (paragraph-ish) and between sections. */
const CHUNK_GAP_SEC = 0.7;
const SECTION_GAP_SEC = 1.5;

const INTRO_PATH = "assets/press-pods/intro.mp3";

export interface SynthesisResult {
  audio: Buffer;
  voiceName: string;
  voiceProvider: string;
  synthesizedSeconds: number;
  chapters: Chapter[];
}

export async function synthesizeSpeech({
  content,
  authorGender,
  logger,
  costCounter,
}: {
  content: string;
  authorGender: MetadataInfo["authorGender"];
  logger: Logger;
  costCounter: CostCounter;
}): Promise<SynthesisResult> {
  const start = Date.now();
  const voice = getVoice(authorGender);
  const sections = splitSections(content);

  logger.info("Starting speech synthesis", {
    voice: voice.name,
    model: TTS_MODEL,
    totalChars: content.length,
    sections: sections.length,
  });

  const introBuffer = await fsAsync.readFile(INTRO_PATH);
  const introDuration = await probeDurationSeconds(INTRO_PATH);

  const chunkGap = await makeSilenceWav(CHUNK_GAP_SEC);
  const sectionGap = await makeSilenceWav(SECTION_GAP_SEC);
  const wavPaths: string[] = [];
  const chapters: Chapter[] = [];
  // Offset into the speech track (excludes the intro jingle, added below).
  let speechOffset = 0;
  let chunkIndex = 0;
  const totalChunks = sections.reduce(
    (n, s) => n + chunkText(s.body, CHUNK_TARGET, CHUNK_MAX).length,
    0,
  );

  try {
    for (let s = 0; s < sections.length; s++) {
      const section = sections[s];
      if (s > 0) {
        wavPaths.push(sectionGap);
        speechOffset += SECTION_GAP_SEC;
      }
      // Chapters only make sense when the article actually had sections.
      if (sections.length > 1) {
        chapters.push({
          startTimeSeconds: introDuration + speechOffset,
          title: section.title ?? "Introduction",
        });
      }

      const chunks = chunkText(section.body, CHUNK_TARGET, CHUNK_MAX);
      for (let i = 0; i < chunks.length; i++) {
        if (i > 0) {
          wavPaths.push(chunkGap);
          speechOffset += CHUNK_GAP_SEC;
        }
        const mp3 = await synthesizeChunk(chunks[i], voice, logger);
        costCounter.recordTtsUsage(TTS_MODEL, "tts", chunks[i]);
        const { wavPath, durationSeconds } = await prepareChunk(mp3);
        wavPaths.push(wavPath);
        speechOffset += durationSeconds;
        chunkIndex++;
        logger.info(`Synthesized chunk ${chunkIndex}/${totalChunks}`);
      }
    }

    const audio = await assembleEpisode(wavPaths, introBuffer);
    logger.info("Speech synthesized", {
      audioBytes: audio.length,
      chapters: chapters.length,
    });

    return {
      audio,
      voiceName: voice.name,
      voiceProvider: "ElevenLabs",
      synthesizedSeconds: (Date.now() - start) / 1000,
      chapters,
    };
  } finally {
    await cleanupWavs([...wavPaths, chunkGap, sectionGap]);
  }
}

/** POST one chunk to ElevenLabs, retrying transient (429/5xx) failures. */
async function synthesizeChunk(
  text: string,
  voice: Voice,
  logger: Logger,
): Promise<Buffer> {
  const apiKey = config.ELEVENLABS_API_KEY;
  if (!apiKey) throw new Error("ELEVENLABS_API_KEY is not set");

  const maxAttempts = 3;
  for (let attempt = 1; ; attempt++) {
    try {
      const bytes = await got
        .post(`${TTS_ENDPOINT}/${voice.id}?output_format=${OUTPUT_FORMAT}`, {
          headers: { "xi-api-key": apiKey },
          json: {
            text,
            model_id: TTS_MODEL,
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
