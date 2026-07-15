import { describe, expect, it } from "vitest";
import { computeComposite, formatCandidateDetails } from "./shortlist.js";
import { type Candidate, CandidateSource, MediaType } from "./types.js";

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

describe("formatCandidateDetails", () => {
  it("exposes viewing commitment and useful structured context to the models", () => {
    const candidate: Candidate = {
      canonicalId: "tmdb:tv:123",
      tmdbId: 123,
      mediaType: MediaType.Tv,
      title: "A Show",
      overview: "",
      genres: ["Drama"],
      voteAverage: 8,
      voteCount: 1_000,
      popularity: 10,
      source: CandidateSource.Similar,
      inLibrary: false,
      runtimeMinutes: 52,
      seasonCount: 4,
      episodeCount: 40,
      seriesStatus: "Ended",
      certification: "TV-MA",
      creators: ["A Creator"],
      cast: ["One", "Two"],
      keywords: ["mystery", "workplace"],
    };

    expect(formatCandidateDetails(candidate)).toContain("52 min/episode");
    expect(formatCandidateDetails(candidate)).toContain("4 seasons");
    expect(formatCandidateDetails(candidate)).toContain("40 episodes");
    expect(formatCandidateDetails(candidate)).toContain("creator=A Creator");
    expect(formatCandidateDetails(candidate)).toContain("themes=mystery, workplace");
  });
});
