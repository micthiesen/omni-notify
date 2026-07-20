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
  SPEED_MULTIPLIER,
} from "./audioChain.js";
import { type CoverageResult, computeCoverage, isContentComplete } from "./coverage.js";
import { createTtsProvider } from "./providers/index.js";
import type { AuthorGender, TtsProvider } from "./providers/types.js";
import { createSttClient, type SttClient } from "./stt.js";
import { chunkText, splitSections } from "./textChunking.js";

/** Plausible narration pacing (seconds of *trimmed*, sped audio per input
 * char). The band is wide on purpose and is the *fallback* verifier (used when
 * STT content-verification is unavailable): it catches catastrophic truncation
 * (a few seconds for a full chunk) and runaway looping (minutes for one
 * paragraph), not a naturally fast reader. Divided by SPEED_MULTIPLIER because
 * prepareChunk speeds the audio, shortening every duration. */
const MIN_SEC_PER_CHAR = 0.03 / SPEED_MULTIPLIER;
const MAX_SEC_PER_CHAR = 0.15 / SPEED_MULTIPLIER;
/** Only ranks fallback takes when all attempts are out of bounds; a complete
 * read sits ~0.06 (Higgs runs faster, ~0.04) before the speed-up. */
const IDEAL_SEC_PER_CHAR = 0.06 / SPEED_MULTIPLIER;
const MAX_SYNTH_ATTEMPTS = 3;
/** Below this the verifiers are dominated by fixed overhead (warm-up, edge
 * silence, STT word-count noise) and can't distinguish truncation, so the
 * check is skipped. */
const MIN_VERIFY_CHARS = 120;

/**
 * Synthesize one chunk and prepare it for concat, re-synthesizing when a
 * verifier rejects the take. Higgs truncates/loops unpredictably; the primary
 * verifier is an STT round-trip (word coverage — see coverage.ts), which
 * cleanly separates a truncated read from a fast one where duration alone can't.
 * When no STT endpoint is configured it falls back to the duration band. The
 * best take is kept even if none pass. A synth/prepare/STT failure counts as a
 * spent attempt rather than failing the whole episode; discarded takes' temp
 * files are cleaned up.
 */
interface ChunkSynthesisOutcome {
  chunk: PreparedChunk;
  /** Synth takes spent (verify retries count; 1 when verification is skipped). */
  attempts: number;
  coverage?: CoverageResult;
}

/** A verifier's read on one take. `verified` marks a real STT content check
 * (vs the duration-band fallback used when STT is off or erroring). `score`
 * ranks least-bad takes when none pass, but is only comparable within one kind
 * — never rank a duration `score` against a content `score` (different axes). */
interface Assessment {
  accept: boolean;
  verified: boolean;
  score: number;
  coverage?: CoverageResult;
  describe: () => string;
}

async function assessTake(
  take: PreparedChunk,
  rawMp3: Buffer,
  text: string,
  stt: SttClient | null,
  useContent: boolean,
  logger: Logger,
): Promise<Assessment> {
  if (useContent && stt) {
    try {
      const transcript = await stt.transcribe(rawMp3, logger);
      const coverage = computeCoverage(text, transcript);
      return {
        accept: isContentComplete(coverage),
        verified: true,
        // Penalize runaway (ratio > 1) as much as truncation so best-of doesn't
        // pick a loop; score peaks at ratio 1.
        score: coverage.coverage - Math.max(0, coverage.wordRatio - 1),
        coverage,
        describe: () =>
          `coverage=${(coverage.coverage * 100).toFixed(0)}% ratio=${coverage.wordRatio.toFixed(2)}`,
      };
    } catch (error) {
      // STT down/erroring: fall through to the duration band for this take so a
      // flaky ASR server never blocks synthesis.
      logger.warn(
        `STT verify failed (${(error as Error).message}); falling back to duration check`,
      );
    }
  }
  const ratio = take.durationSeconds / text.length;
  return {
    accept: ratio >= MIN_SEC_PER_CHAR && ratio <= MAX_SEC_PER_CHAR,
    verified: false,
    score: -Math.abs(ratio - IDEAL_SEC_PER_CHAR),
    describe: () => `${take.durationSeconds.toFixed(1)}s for ${text.length} chars`,
  };
}

async function synthesizeChunkAudio(
  provider: TtsProvider,
  text: string,
  stt: SttClient | null,
  logger: Logger,
): Promise<ChunkSynthesisOutcome> {
  const opts = { denoise: provider.needsDenoise };
  const synth = async (): Promise<{ chunk: PreparedChunk; raw: Buffer }> => {
    const raw = await provider.synthesizeChunk(text, logger);
    return { chunk: await prepareChunk(raw, opts), raw };
  };

  const useContent = provider.verifyChunkContent && stt !== null;
  const verify = provider.verifyChunkLength || useContent;
  if (!verify || text.length < MIN_VERIFY_CHARS) {
    return { chunk: (await synth()).chunk, attempts: 1 };
  }

  const takes: Array<{ chunk: PreparedChunk; assessment: Assessment }> = [];
  let attemptsMade = 0;
  for (let i = 1; i <= MAX_SYNTH_ATTEMPTS; i++) {
    attemptsMade = i;
    try {
      const { chunk, raw } = await synth();
      const assessment = await assessTake(chunk, raw, text, stt, useContent, logger);
      takes.push({ chunk, assessment });
      // Only stop early on a *verified* accept when content-verification is the
      // intended mode — a duration-band accept from a transiently-failed STT
      // call must not short-circuit it (that's the truncation blind spot STT
      // closes). Without content mode, a duration accept is the real bar.
      if (assessment.accept && (assessment.verified || !useContent)) break;
      if (i < MAX_SYNTH_ATTEMPTS) {
        logger.warn(
          `Chunk verify failed (${assessment.describe()}); retry ${i}/${MAX_SYNTH_ATTEMPTS}`,
        );
      }
    } catch (error) {
      // A corrupt/truncated response can fail prepareChunk's ffmpeg; retry
      // rather than aborting the episode.
      logger.warn(
        `Chunk synth/prepare failed (attempt ${i}/${MAX_SYNTH_ATTEMPTS}): ${(error as Error).message}`,
      );
    }
  }

  if (takes.length === 0) {
    throw new Error(`All ${MAX_SYNTH_ATTEMPTS} synthesis attempts failed for a chunk`);
  }
  // When content-verification was intended and at least one take got a real STT
  // read, choose only among those — scores across kinds aren't comparable, and
  // an unverified (duration-only) take must never be preferred to a verified
  // one. Fall back to the full set only if every take's STT call failed.
  const verifiedTakes = takes.filter((t) => t.assessment.verified);
  const pool = useContent && verifiedTakes.length > 0 ? verifiedTakes : takes;
  const chosen =
    pool.find((t) => t.assessment.accept) ??
    pool.reduce((a, b) => (a.assessment.score >= b.assessment.score ? a : b));
  await cleanupWavs(takes.filter((t) => t !== chosen).map((t) => t.chunk.wavPath));
  if (useContent && verifiedTakes.length === 0) {
    logger.warn(
      `Content verification unavailable for every take (STT failing); shipping the ` +
        `duration-best take (${chosen.assessment.describe()}) — truncation may slip through`,
    );
  } else if (!chosen.assessment.accept) {
    logger.warn(
      `Chunk still failing verification after ${MAX_SYNTH_ATTEMPTS} tries; ` +
        `using best take (${chosen.assessment.describe()})`,
    );
  }
  return {
    chunk: chosen.chunk,
    attempts: attemptsMade,
    coverage: chosen.assessment.coverage,
  };
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
  const stt = provider.verifyChunkContent ? createSttClient() : null;
  const sections = splitSections(content);

  logger.info("Starting speech synthesis", {
    provider: provider.providerName,
    voice: provider.voiceName,
    model: provider.modelId,
    totalChars: content.length,
    sections: sections.length,
    contentVerify: stt ? stt.modelId : "off",
  });
  if (provider.verifyChunkContent && !stt) {
    logger.warn(
      "Content verification unavailable (no PRESSPODS_STT_URL / PRESSPODS_TTS_URL); " +
        "falling back to the duration-band check, which lets some truncation through",
    );
  }

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
        const { chunk, attempts, coverage } = await synthesizeChunkAudio(
          provider,
          chunks[i],
          stt,
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
          coverage: coverage?.coverage,
          wordRatio: coverage?.wordRatio,
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
