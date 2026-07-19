import fsAsync from "node:fs/promises";
import type { Logger } from "@micthiesen/mitools/logging";
import type CostCounter from "../costs.js";
import type { Chapter, ChunkStat } from "../types.js";
import {
  assembleEpisode,
  cleanupWavs,
  makeSilenceWav,
  type PreparedChunk,
  prepareChunk,
  probeDurationSeconds,
} from "./audioChain.js";
import { createTtsProvider } from "./providers/index.js";
import type { AuthorGender, TtsProvider } from "./providers/types.js";
import { chunkText, splitSections } from "./textChunking.js";

/** Plausible narration pacing (seconds of *trimmed* audio per input char). The
 * band is wide on purpose: it catches catastrophic truncation (a few seconds
 * for a full chunk) and runaway looping (minutes for one paragraph), not a
 * naturally fast reader. */
const MIN_SEC_PER_CHAR = 0.03;
const MAX_SEC_PER_CHAR = 0.15;
/** Only ranks fallback takes when all attempts are out of bounds; a complete
 * read sits ~0.06 (Higgs runs faster, ~0.04). */
const IDEAL_SEC_PER_CHAR = 0.06;
const MAX_LENGTH_ATTEMPTS = 3;
/** Below this the pacing ratio is dominated by fixed overhead (warm-up, edge
 * silence) and can't distinguish truncation, so the check is skipped. */
const MIN_VERIFY_CHARS = 120;

/**
 * Synthesize one chunk and prepare it for concat. For providers that need it
 * (local models truncate or loop unpredictably), re-synthesize when the
 * prepared (trimmed) audio's pacing is implausible, keeping the take closest to
 * a natural pace. A synth/prepare failure counts as a spent attempt rather than
 * failing the whole episode. Discarded takes' temp files are cleaned up.
 */
interface ChunkSynthesisOutcome {
  chunk: PreparedChunk;
  /** Synth takes spent (length-verify retries count; 1 when skipped). */
  attempts: number;
}

async function synthesizeChunkAudio(
  provider: TtsProvider,
  text: string,
  logger: Logger,
): Promise<ChunkSynthesisOutcome> {
  const opts = { denoise: provider.needsDenoise };
  const attempt = async (): Promise<PreparedChunk> =>
    prepareChunk(await provider.synthesizeChunk(text, logger), opts);

  if (!provider.verifyChunkLength || text.length < MIN_VERIFY_CHARS) {
    return { chunk: await attempt(), attempts: 1 };
  }

  const inBounds = (take: PreparedChunk): boolean => {
    const ratio = take.durationSeconds / text.length;
    return ratio >= MIN_SEC_PER_CHAR && ratio <= MAX_SEC_PER_CHAR;
  };
  const takes: PreparedChunk[] = [];
  let attemptsMade = 0;
  for (let i = 1; i <= MAX_LENGTH_ATTEMPTS; i++) {
    attemptsMade = i;
    try {
      const take = await attempt();
      takes.push(take);
      if (inBounds(take)) break;
      if (i < MAX_LENGTH_ATTEMPTS) {
        logger.warn(
          `Chunk length implausible (${take.durationSeconds.toFixed(1)}s for ` +
            `${text.length} chars); retry ${i}/${MAX_LENGTH_ATTEMPTS}`,
        );
      }
    } catch (error) {
      // A corrupt/truncated response can fail prepareChunk's ffmpeg; retry
      // rather than aborting the episode.
      logger.warn(
        `Chunk synth/prepare failed (attempt ${i}/${MAX_LENGTH_ATTEMPTS}): ${(error as Error).message}`,
      );
    }
  }

  if (takes.length === 0) {
    throw new Error(`All ${MAX_LENGTH_ATTEMPTS} synthesis attempts failed for a chunk`);
  }
  const distance = (take: PreparedChunk): number =>
    Math.abs(take.durationSeconds / text.length - IDEAL_SEC_PER_CHAR);
  const chosen =
    takes.find(inBounds) ??
    takes.reduce((a, b) => (distance(a) <= distance(b) ? a : b));
  await cleanupWavs(takes.filter((t) => t !== chosen).map((t) => t.wavPath));
  if (!inBounds(chosen)) {
    logger.warn(
      `Chunk still implausible after ${MAX_LENGTH_ATTEMPTS} tries; using closest take ` +
        `(${chosen.durationSeconds.toFixed(1)}s)`,
    );
  }
  return { chunk: chosen, attempts: attemptsMade };
}

/** ~900-char chunks keep each request in the quality sweet spot; sections and
 * paragraphs are never split mid-sentence. */
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
  chunks: ChunkStat[];
}

export async function synthesizeSpeech({
  content,
  authorGender,
  logger,
  costCounter,
}: {
  content: string;
  authorGender: AuthorGender;
  logger: Logger;
  costCounter: CostCounter;
}): Promise<SynthesisResult> {
  const start = Date.now();
  const provider = createTtsProvider(authorGender);
  const sections = splitSections(content);

  logger.info("Starting speech synthesis", {
    provider: provider.providerName,
    voice: provider.voiceName,
    model: provider.modelId,
    totalChars: content.length,
    sections: sections.length,
  });

  const introBuffer = await fsAsync.readFile(INTRO_PATH);
  const introDuration = await probeDurationSeconds(INTRO_PATH);

  const chunkGap = await makeSilenceWav(CHUNK_GAP_SEC);
  const sectionGap = await makeSilenceWav(SECTION_GAP_SEC);
  const wavPaths: string[] = [];
  const chapters: Chapter[] = [];
  const chunkStats: ChunkStat[] = [];
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
        const chunkStartTimeSeconds = introDuration + speechOffset;
        const { chunk, attempts } = await synthesizeChunkAudio(
          provider,
          chunks[i],
          logger,
        );
        const { wavPath, durationSeconds } = chunk;
        costCounter.recordTtsUsage(provider.modelId, "tts", chunks[i]);
        wavPaths.push(wavPath);
        speechOffset += durationSeconds;
        chunkIndex++;
        chunkStats.push({
          index: chunkIndex - 1,
          sectionIndex: s,
          sectionTitle: section.title,
          text: chunks[i],
          charCount: chunks[i].length,
          durationSeconds,
          startTimeSeconds: chunkStartTimeSeconds,
          secPerChar: chunks[i].length > 0 ? durationSeconds / chunks[i].length : 0,
          attempts,
        });
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
      voiceName: provider.voiceName,
      voiceProvider: provider.providerName,
      synthesizedSeconds: (Date.now() - start) / 1000,
      chapters,
      chunks: chunkStats,
    };
  } finally {
    await cleanupWavs([...wavPaths, chunkGap, sectionGap]);
  }
}
