import { describe, expect, it, vi } from "vitest";
import { it as effectIt } from "@effect/vitest";
import { Duration, Effect, Fiber } from "effect";
import { TestClock } from "effect/testing";
import type { Metadata } from "../agents/metadata.js";
import { PressPodsError } from "../effect.js";
import type { Article } from "../types.js";
import {
  getArticleRetrievers,
  rateRetrievedArticles,
  runArticleRetrievers,
} from "./index.js";

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
    const rateArticle = vi.fn(() => Effect.succeed(metadata()));

    const results = await Effect.runPromise(
      rateRetrievedArticles(
        [
          { success: true, article: first, retrieverName: "first" },
          { success: false, error: new Error("network"), retrieverName: "failed" },
          { success: true, article: second, retrieverName: "second" },
          { success: true, article: distinct, retrieverName: "distinct" },
        ],
        rateArticle,
      ),
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
    const rateArticle = vi.fn(() => Effect.succeed(metadata()));

    await Effect.runPromise(
      rateRetrievedArticles(
        [
          { success: true, article: first, retrieverName: "first" },
          { success: true, article: second, retrieverName: "second" },
        ],
        rateArticle,
      ),
    );

    expect(rateArticle).toHaveBeenCalledTimes(2);
  });

  it("fans invalid metadata and rating errors out to each matching retriever", async () => {
    const invalidA = article("invalid", "invalid-a");
    const invalidB = { ...invalidA };
    const brokenA = article("broken", "broken-a");
    const brokenB = { ...brokenA };
    const rateArticle = vi.fn((candidate: Article) =>
      candidate.text === "broken"
        ? Effect.fail(
            new PressPodsError({
              operation: "rate article",
              cause: new Error("model unavailable"),
            }),
          )
        : Effect.succeed(metadata(false)),
    );

    const results = await Effect.runPromise(
      rateRetrievedArticles(
        [
          { success: true, article: invalidA, retrieverName: "invalid-a" },
          { success: true, article: brokenA, retrieverName: "broken-a" },
          { success: true, article: invalidB, retrieverName: "invalid-b" },
          { success: true, article: brokenB, retrieverName: "broken-b" },
        ],
        rateArticle,
      ),
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

describe("getArticleRetrievers", () => {
  it("uses the specialized retriever for X and Twitter status URLs", () => {
    expect(
      getArticleRetrievers(
        "https://x.com/edels0n/status/2077031491045929255?s=46&t=share",
      ).map(({ name }) => name),
    ).toEqual(["x"]);
    expect(
      getArticleRetrievers(
        "https://mobile.twitter.com/user/status/2079904005652893709",
      ).map(({ name }) => name),
    ).toEqual(["x"]);
    expect(
      getArticleRetrievers("https://x.com/i/status/2079904005652893709").map(
        ({ name }) => name,
      ),
    ).toEqual(["x"]);
    expect(
      getArticleRetrievers("https://x.com/i/web/status/2079904005652893709").map(
        ({ name }) => name,
      ),
    ).toEqual(["x"]);
  });

  it("keeps generic retrievers for non-status and lookalike URLs", () => {
    expect(
      getArticleRetrievers("https://x.com/explore").map(({ name }) => name),
    ).toContain("readability");
    expect(
      getArticleRetrievers("https://x.com.example/status/123").map(({ name }) => name),
    ).toContain("readability");
  });
});

describe("runArticleRetrievers", () => {
  effectIt.effect(
    "interrupts a timed-out child without discarding a successful sibling",
    () =>
      Effect.gen(function* () {
        let interrupted = false;
        const successful = article("complete", "successful");
        const fiber = yield* Effect.forkChild(
          runArticleRetrievers(
            "https://example.com/article",
            [
              {
                name: "hung",
                retrieve: () =>
                  Effect.callback<never>((resume) => {
                    return Effect.sync(() => {
                      interrupted = true;
                      resume(Effect.interrupt);
                    });
                  }),
              },
              { name: "successful", retrieve: () => Effect.succeed(successful) },
            ],
            5,
          ),
        );
        yield* Effect.yieldNow;
        yield* TestClock.adjust(Duration.millis(5));
        const results = yield* Fiber.join(fiber);

        expect(interrupted).toBe(true);
        expect(results).toHaveLength(2);
        expect(results[0]).toMatchObject({ success: false, retrieverName: "hung" });
        expect(results[1]).toEqual({
          success: true,
          article: successful,
          retrieverName: "successful",
        });
      }),
  );
});
