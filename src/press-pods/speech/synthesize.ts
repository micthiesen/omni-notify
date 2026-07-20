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
/** A splittable chunk gets only this many full-size re-rolls before we stop
 * banging on the same text and adapt (re-split). Higgs truncation is
 * length-correlated and non-deterministic, so re-rolling a long chunk rarely
 * recovers — smaller boundary-safe pieces do. Leaf chunks that can't be split
 * still get the full MAX_SYNTH_ATTEMPTS. */
const RESPLIT_PROBE_ATTEMPTS = 2;
/** Don't re-split a chunk already this short: the sub-pieces would be tiny and
 * a residual failure at this size is a content/STT quirk, not truncation. */
const MIN_RESPLIT_CHARS = 500;
/** Target size for re-split sub-chunks — comfortably inside Higgs's reliable
 * range so a first-level split usually clears verification. */
const RESPLIT_TARGET_CHARS = 400;
/** Safety bound on recursive re-splitting (a pathological all-failing article
 * can't fan out without limit). */
const MAX_RESPLIT_DEPTH = 2;

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
  /** True when the chosen take cleared verification (or verification was
   * skipped/unavailable). False means the best take is still failing — the
   * signal the adaptive caller uses to decide whether to re-split. */
  passed: boolean;
}

/** One synthesized, verified, concat-ready piece of narration. A single input
 * chunk yields one piece normally, or several when adaptive re-splitting kicks
 * in. Carries its own text so cost accounting and ChunkStats stay accurate. */
interface ChunkPiece {
  chunk: PreparedChunk;
  text: string;
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
  maxAttempts: number = MAX_SYNTH_ATTEMPTS,
): Promise<ChunkSynthesisOutcome> {
  const opts = { denoise: provider.needsDenoise };
  const synth = async (): Promise<{ chunk: PreparedChunk; raw: Buffer }> => {
    const raw = await provider.synthesizeChunk(text, logger);
    return { chunk: await prepareChunk(raw, opts), raw };
  };

  const useContent = provider.verifyChunkContent && stt !== null;
  const verify = provider.verifyChunkLength || useContent;
  if (!verify || text.length < MIN_VERIFY_CHARS) {
    return { chunk: (await synth()).chunk, attempts: 1, passed: true };
  }

  const takes: Array<{ chunk: PreparedChunk; assessment: Assessment }> = [];
  let attemptsMade = 0;
  for (let i = 1; i <= maxAttempts; i++) {
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
      if (i < maxAttempts) {
        logger.warn(
          `Chunk verify failed (${assessment.describe()}); retry ${i}/${maxAttempts}`,
        );
      }
    } catch (error) {
      // A corrupt/truncated response can fail prepareChunk's ffmpeg; retry
      // rather than aborting the episode.
      logger.warn(
        `Chunk synth/prepare failed (attempt ${i}/${maxAttempts}): ${(error as Error).message}`,
      );
    }
  }

  if (takes.length === 0) {
    throw new Error(`All ${maxAttempts} synthesis attempts failed for a chunk`);
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
  // STT failing on every take means verification was *unavailable*, not that the
  // audio is truncated — treat it as passed so the caller doesn't re-split
  // pointlessly (the sub-chunks couldn't be verified either).
  const verificationUnavailable = useContent && verifiedTakes.length === 0;
  const passed = verificationUnavailable || chosen.assessment.accept;
  if (verificationUnavailable) {
    logger.warn(
      `Content verification unavailable for every take (STT failing); shipping the ` +
        `duration-best take (${chosen.assessment.describe()}) — truncation may slip through`,
    );
  } else if (!chosen.assessment.accept) {
    logger.warn(
      `Chunk still failing verification after ${attemptsMade} tries ` +
        `(${chosen.assessment.describe()})`,
    );
  }
  return {
    chunk: chosen.chunk,
    attempts: attemptsMade,
    coverage: chosen.assessment.coverage,
    passed,
  };
}

/**
 * Synthesize one narration chunk, adapting on failure. A chunk large enough to
 * split gets a short probe budget of full-size re-rolls; if it still fails
 * verification, we stop re-rolling the same text and re-split it into smaller,
 * boundary-safe sub-chunks (via chunkText, which never cuts mid-sentence) and
 * synthesize each recursively — the reliable path for Higgs's length-correlated
 * truncation. Chunks too short to split (or at the recursion floor) keep the
 * full retry budget and ship the best take even if it never passes. Returns one
 * concat-ready piece per synthesized unit.
 */
async function synthesizeChunkAdaptive(
  provider: TtsProvider,
  text: string,
  stt: SttClient | null,
  logger: Logger,
  depth = 0,
): Promise<ChunkPiece[]> {
  const splittable = depth < MAX_RESPLIT_DEPTH && text.length >= MIN_RESPLIT_CHARS;
  // A splittable chunk that verifies badly re-splits; one that *throws* on every
  // probe (network/ffmpeg/TTS 5xx) should also fall through to re-split rather
  // than abort the whole episode — smaller sub-chunks are the recovery path for
  // both. Only a non-splittable chunk's hard failure propagates. `outcome` is
  // null in that fall-through case.
  let outcome: ChunkSynthesisOutcome | null = null;
  try {
    outcome = await synthesizeChunkAudio(
      provider,
      text,
      stt,
      logger,
      splittable ? RESPLIT_PROBE_ATTEMPTS : MAX_SYNTH_ATTEMPTS,
    );
  } catch (error) {
    if (!splittable) throw error;
    logger.warn(
      `Chunk synthesis threw on every probe (${(error as Error).message}); ` +
        `re-splitting to recover`,
    );
  }

  if (outcome && (outcome.passed || !splittable)) {
    return [
      {
        chunk: outcome.chunk,
        text,
        attempts: outcome.attempts,
        coverage: outcome.coverage,
      },
    ];
  }

  const subChunks = chunkText(text, RESPLIT_TARGET_CHARS, RESPLIT_TARGET_CHARS);
  // A single unsplittable sentence can't be broken without a mid-sentence cut.
  // Keep the best take if we have one; if the chunk hard-failed and can't split,
  // there's nothing to ship — let the error propagate.
  if (subChunks.length <= 1) {
    if (outcome) {
      return [
        {
          chunk: outcome.chunk,
          text,
          attempts: outcome.attempts,
          coverage: outcome.coverage,
        },
      ];
    }
    throw new Error(`Chunk synthesis failed and the text could not be re-split`);
  }

  logger.warn(
    `Re-splitting failing chunk (${text.length} chars) into ${subChunks.length} ` +
      `boundary-safe sub-chunks and re-synthesizing`,
  );
  if (outcome) await cleanupWavs([outcome.chunk.wavPath]);
  // Sub-chunks come from splitting one contiguous chunk on sentence/paragraph
  // boundaries, so the chunk gap the caller inserts between the resulting pieces
  // lands at a natural boundary — an accepted trade for the reliability gain.
  const pieces: ChunkPiece[] = [];
  try {
    for (const sub of subChunks) {
      pieces.push(
        ...(await synthesizeChunkAdaptive(provider, sub, stt, logger, depth + 1)),
      );
    }
  } catch (error) {
    // A later sibling hard-failed: earlier siblings' kept WAVs were never handed
    // to the caller's wavPaths, so clean them up here or they leak in tmpdir.
    await cleanupWavs(pieces.map((p) => p.chunk.wavPath));
    throw error;
  }
  return pieces;
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
  // Pre-split estimate; grows as adaptive re-splitting turns one chunk into
  // several, so the progress fraction stays honest instead of pinning at N/N.
  let totalChunks = sections.reduce(
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
      let firstPieceInSection = true;
      for (const chunk of chunks) {
        // One input chunk yields one piece, or several when adaptive re-splitting
        // breaks a chunk that kept failing verification into smaller pieces.
        const pieces = await synthesizeChunkAdaptive(provider, chunk, stt, logger);
        // A re-split adds pieces beyond the pre-split estimate; keep total honest.
        totalChunks += pieces.length - 1;
        for (const piece of pieces) {
          if (!firstPieceInSection) {
            wavPaths.push(chunkGap);
            speechOffset += CHUNK_GAP_SEC;
          }
          firstPieceInSection = false;
          const chunkStartTimeSeconds = introDuration + speechOffset;
          const { wavPath, durationSeconds } = piece.chunk;
          costCounter.recordTtsUsage(provider.modelId, "tts", piece.text);
          wavPaths.push(wavPath);
          speechOffset += durationSeconds;
          chunkIndex++;
          chunkStats.push({
            index: chunkIndex - 1,
            sectionIndex: s,
            sectionTitle: section.title,
            text: piece.text,
            charCount: piece.text.length,
            durationSeconds,
            startTimeSeconds: chunkStartTimeSeconds,
            secPerChar: piece.text.length > 0 ? durationSeconds / piece.text.length : 0,
            attempts: piece.attempts,
            coverage: piece.coverage?.coverage,
            wordRatio: piece.coverage?.wordRatio,
          });
          logger.info(`Synthesized chunk ${chunkIndex}/${totalChunks}`);
        }
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
