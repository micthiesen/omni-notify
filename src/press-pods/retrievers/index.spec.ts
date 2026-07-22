import { describe, expect, it, vi } from "vitest";
import type { Metadata } from "../agents/metadata.js";
import type { Article } from "../types.js";
import { rateRetrievedArticles } from "./index.js";

function article(text: string, title: string): Article {
  return {
    title,
    text,
    author: undefined,
    domain: "example.com",
    url: `https://example.com/${title}`,
    publishedAt: undefined,
    leadImageUrl: undefined,
  };
}

function metadata(isValidArticle = true, contentRating = 9): Metadata {
  return {
    info: {
      isValidArticle,
      title: "Rated title",
      author: undefined,
      authorGender: "unknown",
      coauthors: null,
      publication: "Example",
      publishedAtISO: undefined,
      leadImageUrl: null,
      shortSummary: "Summary",
      contentRating,
    },
  };
}

describe("rateRetrievedArticles", () => {
  it("rates normalized exact text once and preserves provider order and articles", async () => {
    const first = article("same\r\ntext\n", "first");
    const second = { ...first, text: " same\ntext" };
    const distinct = article("same text", "distinct");
    const rateArticle = vi.fn(async () => metadata());

    const results = await rateRetrievedArticles(
      [
        { success: true, article: first, retrieverName: "first" },
        { success: false, error: new Error("network"), retrieverName: "failed" },
        { success: true, article: second, retrieverName: "second" },
        { success: true, article: distinct, retrieverName: "distinct" },
      ],
      rateArticle,
    );

    expect(rateArticle).toHaveBeenCalledTimes(2);
    expect(rateArticle).toHaveBeenNthCalledWith(1, first);
    expect(rateArticle).toHaveBeenNthCalledWith(2, distinct);
    expect(results.map((result) => result.retrieverName)).toEqual([
      "first",
      "failed",
      "second",
      "distinct",
    ]);
    expect(results[0]).toMatchObject({ success: true, article: first });
    expect(results[1]).toMatchObject({ success: false });
    expect(results[2]).toMatchObject({ success: true, article: second });
    expect(results[3]).toMatchObject({ success: true, article: distinct });
  });

  it("rates matching body text separately when prompt metadata differs", async () => {
    const first = article("same text", "first");
    const second = article("same text", "second");
    const rateArticle = vi.fn(async () => metadata());

    await rateRetrievedArticles(
      [
        { success: true, article: first, retrieverName: "first" },
        { success: true, article: second, retrieverName: "second" },
      ],
      rateArticle,
    );

    expect(rateArticle).toHaveBeenCalledTimes(2);
  });

  it("fans invalid metadata and rating errors out to each matching retriever", async () => {
    const invalidA = article("invalid", "invalid-a");
    const invalidB = { ...invalidA };
    const brokenA = article("broken", "broken-a");
    const brokenB = { ...brokenA };
    const rateArticle = vi.fn(async (candidate: Article) => {
      if (candidate.text === "broken") throw new Error("model unavailable");
      return metadata(false);
    });

    const results = await rateRetrievedArticles(
      [
        { success: true, article: invalidA, retrieverName: "invalid-a" },
        { success: true, article: brokenA, retrieverName: "broken-a" },
        { success: true, article: invalidB, retrieverName: "invalid-b" },
        { success: true, article: brokenB, retrieverName: "broken-b" },
      ],
      rateArticle,
    );

    expect(rateArticle).toHaveBeenCalledTimes(2);
    expect(results).toHaveLength(4);
    expect(results.every((result) => !result.success)).toBe(true);
    expect((results[0] as { error: Error }).error.message).toBe("Invalid article");
    expect((results[2] as { error: Error }).error.message).toBe("Invalid article");
    expect((results[1] as { error: Error }).error.message).toBe("model unavailable");
    expect((results[3] as { error: Error }).error.message).toBe("model unavailable");
  });
});
