import { describe, expect, it } from "vitest";
import {
  computeCoverage,
  DEFAULT_CONTENT_BOUNDS,
  isContentComplete,
} from "./coverage.js";

const COMPLETE = `I strongly believe that written text should be from humans to
humans. Yet, I still write my texts with the help of language models, and I
don't find that contradictory. What distinguishes AI slop from good writing is
whether a human has put thought behind it. You cannot outsource thinking.`;

describe("computeCoverage", () => {
  it("scores an exact transcript as full coverage", () => {
    const r = computeCoverage(COMPLETE, COMPLETE);
    expect(r.coverage).toBe(1);
    expect(r.wordRatio).toBeCloseTo(1, 5);
  });

  it("tolerates casing and punctuation differences (STT vs cleaner)", () => {
    const stt = COMPLETE.replace(/AI/g, "A.I.").toUpperCase();
    const r = computeCoverage(COMPLETE, stt);
    expect(r.coverage).toBeGreaterThanOrEqual(0.9);
  });

  it("detects truncation as low coverage", () => {
    // Only the first ~40% of the words made it into the audio.
    const words = COMPLETE.split(/\s+/);
    const truncated = words.slice(0, Math.floor(words.length * 0.4)).join(" ");
    const r = computeCoverage(COMPLETE, truncated);
    expect(r.coverage).toBeLessThan(0.6);
    expect(r.wordRatio).toBeLessThan(0.6);
    expect(isContentComplete(r)).toBe(false);
  });

  it("passes a complete read under the default bounds", () => {
    const r = computeCoverage(COMPLETE, COMPLETE);
    expect(isContentComplete(r)).toBe(true);
  });

  it("flags a runaway loop via wordRatio even at full coverage", () => {
    const looped = `${COMPLETE} ${COMPLETE} ${COMPLETE}`;
    const r = computeCoverage(COMPLETE, looped);
    expect(r.coverage).toBe(1);
    expect(r.wordRatio).toBeGreaterThan(DEFAULT_CONTENT_BOUNDS.maxWordRatio);
    expect(isContentComplete(r)).toBe(false);
  });

  it("treats empty expected text as complete (nothing to cover)", () => {
    const r = computeCoverage("", "anything");
    expect(r.coverage).toBe(1);
    expect(isContentComplete(r)).toBe(true);
  });

  it("reflects the real truncated-chunk measurements", () => {
    // From production: truncated Higgs chunks measured 0.36–0.47 wordRatio and
    // ≤0.66 coverage; complete reads ~1.0. The bar must reject the former.
    expect(
      isContentComplete({
        coverage: 0.48,
        wordRatio: 0.36,
        expectedWords: 146,
        transcriptWords: 52,
      }),
    ).toBe(false);
    expect(
      isContentComplete({
        coverage: 0.66,
        wordRatio: 0.47,
        expectedWords: 119,
        transcriptWords: 56,
      }),
    ).toBe(false);
    expect(
      isContentComplete({
        coverage: 1.0,
        wordRatio: 1.0,
        expectedWords: 51,
        transcriptWords: 52,
      }),
    ).toBe(true);
  });
});
