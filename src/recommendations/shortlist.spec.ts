import { describe, expect, it } from "vitest";
import { computeComposite } from "./shortlist.js";

describe("computeComposite", () => {
  it("weights taste match most heavily", () => {
    const tasteHeavy = computeComposite({
      tasteMatch: 90,
      novelty: 50,
      effortFit: 50,
      confidence: 1,
    });
    const noveltyHeavy = computeComposite({
      tasteMatch: 50,
      novelty: 90,
      effortFit: 50,
      confidence: 1,
    });
    expect(tasteHeavy).toBeGreaterThan(noveltyHeavy);
  });

  it("shrinks scores toward the middle at low confidence", () => {
    const confident = computeComposite({
      tasteMatch: 80,
      novelty: 80,
      effortFit: 80,
      confidence: 1,
    });
    const unsure = computeComposite({
      tasteMatch: 80,
      novelty: 80,
      effortFit: 80,
      confidence: 0,
    });
    expect(unsure).toBe(confident / 2);
  });

  it("is bounded by 0 and 100", () => {
    expect(
      computeComposite({
        tasteMatch: 100,
        novelty: 100,
        effortFit: 100,
        confidence: 1,
      }),
    ).toBeLessThanOrEqual(100);
    expect(
      computeComposite({ tasteMatch: 0, novelty: 0, effortFit: 0, confidence: 0 }),
    ).toBe(0);
  });
});
