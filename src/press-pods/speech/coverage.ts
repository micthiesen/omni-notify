/**
 * Content-completeness scoring for a synthesized chunk, by comparing an STT
 * transcript of the audio against the text that was sent to TTS. Autoregressive
 * models (Higgs) silently truncate — emitting a natural-sounding read of only
 * the first half of a chunk — which a duration check can't reliably catch (a
 * truncated read and a naturally fast read overlap in seconds-per-char). Word
 * coverage separates them cleanly: a complete read recovers ~all the input
 * words, a truncated one recovers a fraction. Pure + deterministic for testing.
 */

/** Normalize to a bag of comparable word tokens (case/punctuation-insensitive). */
function tokenize(text: string): string[] {
  return text
    .toLowerCase()
    .replace(/[^a-z0-9\s]/g, " ")
    .replace(/\s+/g, " ")
    .trim()
    .split(" ")
    .filter((w) => w.length > 0);
}

export interface CoverageResult {
  /** Fraction of the distinct expected words that appear in the transcript. */
  coverage: number;
  /**
   * Transcript word count / expected word count. ~1 for a complete read, well
   * below 1 for truncation, well above 1 for a runaway loop (which repeats
   * words, so `coverage` stays high while this blows up).
   */
  wordRatio: number;
  expectedWords: number;
  transcriptWords: number;
}

export function computeCoverage(expected: string, transcript: string): CoverageResult {
  const expectedTokens = tokenize(expected);
  const transcriptTokens = tokenize(transcript);
  if (expectedTokens.length === 0) {
    return { coverage: 1, wordRatio: 1, expectedWords: 0, transcriptWords: 0 };
  }
  const expectedSet = new Set(expectedTokens);
  const transcriptSet = new Set(transcriptTokens);
  let present = 0;
  for (const w of expectedSet) if (transcriptSet.has(w)) present++;
  return {
    coverage: present / expectedSet.size,
    wordRatio: transcriptTokens.length / expectedTokens.length,
    expectedWords: expectedTokens.length,
    transcriptWords: transcriptTokens.length,
  };
}

export interface ContentBounds {
  /** Minimum word coverage for a read to count as complete. */
  minCoverage: number;
  /** Maximum word ratio before a read is treated as a runaway loop. */
  maxWordRatio: number;
}

/**
 * Measured separation (Higgs, self-hosted): complete reads land at ~1.0
 * coverage / ~1.0 ratio; truncated reads at ≤0.66 coverage / ≤0.47 ratio. The
 * 0.75 / 1.8 bar sits in the wide gap between the two populations.
 */
export const DEFAULT_CONTENT_BOUNDS: ContentBounds = {
  minCoverage: 0.75,
  maxWordRatio: 1.8,
};

export function isContentComplete(
  result: CoverageResult,
  bounds: ContentBounds = DEFAULT_CONTENT_BOUNDS,
): boolean {
  return (
    result.coverage >= bounds.minCoverage && result.wordRatio <= bounds.maxWordRatio
  );
}
