import fsAsync from "node:fs/promises";
import type { Logger } from "@micthiesen/mitools/logging";
import type CostCounter from "../costs.js";
import { isRetryableError } from "../errors.js";
import {
  checkpointKey,
  deleteChunkCheckpoint,
  materializeCheckpointWav,
  readChunkCheckpoint,
  writeChunkCheckpoint,
} from "../storage.js";
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
import { chunkText, splitChunkForRetry, splitSections } from "./textChunking.js";

/**
 * Per-article checkpoint context threaded through synthesis. When present, each
 * verified chunk's prepared WAV is cached on disk (keyed by render signature +
 * chunk text) so an abrupt restart resumes from the last good chunk instead of
 * re-synthesizing everything. Null disables checkpointing (e.g. no work id).
 */
interface CheckpointCtx {
  workId: string;
  signature: string;
}

/** Everything about the render that changes the produced audio bytes — folded
 * into the checkpoint key so a mid-flight voice/provider/speed change can never
 * serve a stale take from a previous attempt. */
function renderSignature(provider: TtsProvider): string {
  return [
    provider.providerName,
    provider.voiceName,
    provider.modelId,
    provider.needsDenoise ? "dn" : "raw",
    `x${SPEED_MULTIPLIER}`,
  ].join("|");
}

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
/** A splittable chunk gets only this many full-size attempts before we stop
 * banging on the same text and adapt (re-split). Higgs truncation is
 * length-correlated and non-deterministic, so a second full-size re-roll of a
 * long chunk almost always fails the same way — re-splitting into smaller
 * boundary-safe pieces is the reliable recovery, so go there immediately after
 * the first verification failure. Leaf chunks that can't be split still get the
 * full MAX_SYNTH_ATTEMPTS. */
const RESPLIT_PROBE_ATTEMPTS = 1;
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
  /** Set when this piece is a sub-chunk of a re-split (parent kept failing). */
  resplit?: boolean;
  /** Number of adaptive re-split levels used to produce this piece. */
  resplitDepth?: number;
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
  costCounter: CostCounter,
  logger: Logger,
  ckpt: CheckpointCtx | null,
  maxAttempts: number = MAX_SYNTH_ATTEMPTS,
): Promise<ChunkSynthesisOutcome> {
  const opts = { denoise: provider.needsDenoise };
  const key = ckpt ? checkpointKey(ckpt.signature, text) : null;

  // Resume: a prepared WAV cached from a previous (crashed) attempt skips synth
  // and verification entirely. Probing validates the file is intact; a corrupt
  // checkpoint is treated as a miss so it can never poison the episode.
  if (ckpt && key) {
    const cached = await readChunkCheckpoint(ckpt.workId, key);
    if (cached) {
      let wavPath: string | undefined;
      try {
        wavPath = await materializeCheckpointWav(cached);
        const durationSeconds = await probeDurationSeconds(wavPath);
        logger.info(`Resumed chunk from checkpoint (${text.length} chars)`);
        return { chunk: { wavPath, durationSeconds }, attempts: 0, passed: true };
      } catch (error) {
        // Corrupt/unreadable checkpoint: clean up the temp file it produced and
        // drop the bad entry so it isn't re-probed (and re-leaked) on every
        // future resume, then fall through to synthesize fresh.
        if (wavPath) await cleanupWavs([wavPath]);
        await deleteChunkCheckpoint(ckpt.workId, key);
        logger.warn(
          `Discarded unreadable chunk checkpoint: ${(error as Error).message}`,
        );
      }
    }
  }

  // Cache a verified take so a later restart resumes here. Best-effort: a read
  // failure just costs the resume speedup, never the episode.
  const cache = async (chunk: PreparedChunk): Promise<void> => {
    if (!ckpt || !key) return;
    try {
      await writeChunkCheckpoint(
        ckpt.workId,
        key,
        await fsAsync.readFile(chunk.wavPath),
      );
    } catch {
      // ignore
    }
  };

  const synth = async (): Promise<{ chunk: PreparedChunk; raw: Buffer }> => {
    const raw = await provider.synthesizeChunk(text, logger);
    // Bill every real TTS response here, at the call site — retried and re-split
    // takes are all charged (ElevenLabs bills generated chars even when we later
    // discard the take), so cost tracks actual spend, not just the final pieces.
    costCounter.recordTtsUsage(provider.modelId, "tts", text);
    return { chunk: await prepareChunk(raw, opts), raw };
  };

  const useContent = provider.verifyChunkContent && stt !== null;
  const verify = provider.verifyChunkLength || useContent;
  if (!verify || text.length < MIN_VERIFY_CHARS) {
    const { chunk } = await synth();
    await cache(chunk);
    return { chunk, attempts: 1, passed: true };
  }

  const takes: Array<{ chunk: PreparedChunk; assessment: Assessment }> = [];
  let attemptsMade = 0;
  let lastError: unknown;
  let retryableError: unknown;
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
      lastError = error;
      if (retryableError === undefined && isRetryableError(error)) {
        retryableError = error;
      }
      // A corrupt/truncated response can fail prepareChunk's ffmpeg; retry
      // rather than aborting the episode.
      logger.warn(
        `Chunk synth/prepare failed (attempt ${i}/${maxAttempts}): ${(error as Error).message}`,
      );
    }
  }

  if (takes.length === 0) {
    // Preserve provider error identity (status/code/name) so the durable job
    // queue can recognize transient outages and retry later. Wrapping this in a
    // plain Error would incorrectly turn a retryable network failure permanent.
    throw (
      retryableError ??
      lastError ??
      new Error(`All ${maxAttempts} synthesis attempts failed for a chunk`)
    );
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
  // Only checkpoint a genuinely-validated take. When STT content-verification is
  // the intended mode, a duration-band "pass" from an STT outage must NOT be
  // cached — a later resume skips verification, which would permanently lock in
  // audio a healthy verifier might have rejected as truncated. `passed` stays
  // broader (it drives the re-split decision) so an STT outage doesn't trigger
  // pointless re-splitting; the cache gate is the stricter one.
  const trulyVerified = useContent
    ? chosen.assessment.verified && chosen.assessment.accept
    : chosen.assessment.accept;
  if (trulyVerified) await cache(chosen.chunk);
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
  costCounter: CostCounter,
  logger: Logger,
  ckpt: CheckpointCtx | null,
  depth = 0,
): Promise<ChunkPiece[]> {
  const subChunks = splitChunkForRetry(text, depth);
  const splittable = subChunks !== null;
  // A splittable chunk that verifies badly re-splits. A hard local failure can
  // also recover through smaller chunks, but transient provider failures must
  // propagate to the durable job retry rather than fan out. `outcome` is null
  // when a non-transient hard failure falls through to re-splitting.
  let outcome: ChunkSynthesisOutcome | null = null;
  try {
    outcome = await synthesizeChunkAudio(
      provider,
      text,
      stt,
      costCounter,
      logger,
      ckpt,
      splittable ? RESPLIT_PROBE_ATTEMPTS : MAX_SYNTH_ATTEMPTS,
    );
  } catch (error) {
    // Smaller chunks cannot repair an unavailable/rate-limited server. Let the
    // durable job retry use its backoff and existing verified checkpoints
    // instead of multiplying requests across a re-split tree.
    if (!splittable || isRetryableError(error)) throw error;
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

  // `splittable` guarantees this is non-null; keep the assertion local so the
  // retry plan is computed once and cannot disagree with the probe budget.
  if (!subChunks) throw new Error("Missing adaptive chunk split");

  logger.warn(
    `Re-splitting failing chunk at level ${depth + 1} (${text.length} chars) ` +
      `into ${subChunks.length} ` +
      `boundary-safe sub-chunks and re-synthesizing`,
  );
  if (outcome) await cleanupWavs([outcome.chunk.wavPath]);
  // Sub-chunks come from splitting one contiguous chunk on sentence/paragraph
  // boundaries, so the chunk gap the caller inserts between the resulting pieces
  // lands at a natural boundary — an accepted trade for the reliability gain.
  const pieces: ChunkPiece[] = [];
  try {
    for (const sub of subChunks) {
      const subPieces = await synthesizeChunkAdaptive(
        provider,
        sub,
        stt,
        costCounter,
        logger,
        ckpt,
        depth + 1,
      );
      // Mark every descendant so the UI can show recovery happened here, even
      // when a sub-piece ultimately passed on its first take.
      for (const piece of subPieces) {
        piece.resplit = true;
        piece.resplitDepth = Math.max(piece.resplitDepth ?? 0, depth + 1);
      }
      pieces.push(...subPieces);
    }
  } catch (error) {
    // A later sibling hard-failed: earlier siblings' kept WAVs were never handed
    // to the caller's wavPaths, so clean them up here or they leak in tmpdir.
    await cleanupWavs(pieces.map((p) => p.chunk.wavPath));
    throw error;
  }
  return pieces;
}

interface InitialChunkProfile {
  target: number;
  max: number;
}

/**
 * Higgs failures rise sharply around 700 chars, so providers that require the
 * content-verification path start below that cliff. Reliable providers retain
 * the larger chunks to avoid needless request and seam overhead. Both profiles
 * remain boundary-safe because chunkText never cuts mid-sentence.
 */
const CONTENT_VERIFIED_CHUNK_PROFILE: InitialChunkProfile = {
  target: 650,
  max: 700,
};
const DEFAULT_CHUNK_PROFILE: InitialChunkProfile = {
  target: 900,
  max: 1500,
};

function initialChunkProfile(provider: TtsProvider): InitialChunkProfile {
  return provider.verifyChunkContent
    ? CONTENT_VERIFIED_CHUNK_PROFILE
    : DEFAULT_CHUNK_PROFILE;
}
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
  workId,
}: {
  content: string;
  authorGender: AuthorGender;
  logger: Logger;
  costCounter: CostCounter;
  /** Stable per-article id enabling per-chunk resume across restarts. */
  workId?: string;
}): Promise<SynthesisResult> {
  const start = Date.now();
  const provider = createTtsProvider(authorGender);
  const stt = provider.verifyChunkContent ? createSttClient() : null;
  const ckpt: CheckpointCtx | null = workId
    ? { workId, signature: renderSignature(provider) }
    : null;
  const chunkProfile = initialChunkProfile(provider);
  const sections = splitSections(content);

  logger.info("Starting speech synthesis", {
    provider: provider.providerName,
    voice: provider.voiceName,
    model: provider.modelId,
    totalChars: content.length,
    sections: sections.length,
    contentVerify: stt ? stt.modelId : "off",
    chunkTarget: chunkProfile.target,
    chunkMax: chunkProfile.max,
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
    (n, s) => n + chunkText(s.body, chunkProfile.target, chunkProfile.max).length,
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

      const chunks = chunkText(section.body, chunkProfile.target, chunkProfile.max);
      let firstPieceInSection = true;
      for (const chunk of chunks) {
        // One input chunk yields one piece, or several when adaptive re-splitting
        // breaks a chunk that kept failing verification into smaller pieces.
        const pieces = await synthesizeChunkAdaptive(
          provider,
          chunk,
          stt,
          costCounter,
          logger,
          ckpt,
        );
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
          // Cost is billed per real synth call inside synthesizeChunkAudio, not
          // here — a re-split or retried chunk makes several calls per piece.
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
            expectedWords: piece.coverage?.expectedWords,
            resplit: piece.resplit,
            resplitDepth: piece.resplitDepth,
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
