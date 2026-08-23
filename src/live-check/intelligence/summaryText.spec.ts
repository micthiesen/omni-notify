import { describe, expect, it } from "vitest";
import {
  areSameLivestreamTopic,
  cleanLivestreamSummary,
  cleanLivestreamTopic,
} from "./summaryText.js";

describe("cleanLivestreamSummary", () => {
  it("keeps complete concise summaries unchanged", () => {
    const summary =
      "They compare two melanoma treatments. The discussion turns to side effects.";
    expect(cleanLivestreamSummary(summary)).toBe(summary);
  });

  it("drops constrained-output artifacts and keeps the last complete sentence", () => {
    expect(
      cleanLivestreamSummary(
        "The speaker explains how immunotherapy works. The treatment can cause severe side effects. Because melanoma can depend on免",
      ),
    ).toBe(
      "The speaker explains how immunotherapy works. The treatment can cause severe side effects.",
    );
  });

  it("uses an ellipsis when the model returns only an unfinished sentence", () => {
    expect(
      cleanLivestreamSummary(
        "The discussion compares personalized cancer vaccines with a small pancreatic cancer trial that had noক",
      ),
    ).toBe(
      "The discussion compares personalized cancer vaccines with a small pancreatic cancer trial that had…",
    );
  });

  it("falls back safely when the response is only an artifact", () => {
    expect(cleanLivestreamSummary("####")).toBe(
      "The current discussion could not be summarized cleanly.",
    );
    expect(cleanLivestreamTopic("####")).toBe("Current discussion");
  });
});

describe("cleanLivestreamTopic", () => {
  it("removes artifact tails and respects the compact label limit", () => {
    expect(cleanLivestreamTopic("Personalized mRNA melanoma immunotherapy####")).toBe(
      "Personalized mRNA melanoma immunotherapy",
    );
    expect(
      cleanLivestreamTopic(
        "Debate over Professor Dave's claims about the Al-Ahli hospital explosion and its aftermath",
      ),
    ).toBe("Debate over Professor Dave's claims about the Al-Ahli…");
  });
});

describe("areSameLivestreamTopic", () => {
  it("merges labels with substantial subject overlap", () => {
    expect(
      areSameLivestreamTopic(
        "Melanoma immunotherapy benefits and risks",
        "Benefits and side effects of melanoma immunotherapy",
      ),
    ).toBe(true);
  });

  it("keeps genuinely different subjects separate", () => {
    expect(
      areSameLivestreamTopic(
        "Melanoma immunotherapy benefits and risks",
        "Gaming performance and matchmaking jokes",
      ),
    ).toBe(false);
  });
});
