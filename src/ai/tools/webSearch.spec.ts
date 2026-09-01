import { Effect } from "effect";
import { describe, expect, it, vi } from "vitest";
import type {
  LimitedTextResponse,
  PublicTextRequest,
} from "../../effect/publicHttp.js";
import { searchWebEffect } from "./webSearch.js";

function response(chunks: string[]): LimitedTextResponse {
  return {
    async *[Symbol.asyncIterator]() {
      for (const chunk of chunks) yield chunk;
    },
  };
}

describe("searchWebEffect", () => {
  it("streams and decodes the bounded Tavily response", async () => {
    const request = vi.fn(() =>
      response([
        JSON.stringify({
          results: [
            { title: "Result", url: "https://example.com", content: "excerpt" },
          ],
          response_time: 0.25,
        }),
      ]),
    ) as PublicTextRequest;

    await expect(
      Effect.runPromise(
        searchWebEffect({ query: "test" }, { request, maxResponseBytes: 1024 }),
      ),
    ).resolves.toEqual({
      results: [{ title: "Result", url: "https://example.com", content: "excerpt" }],
      responseTime: 0.25,
    });
    expect(request).toHaveBeenCalledWith(
      "https://api.tavily.com/search",
      expect.objectContaining({ method: "POST", signal: expect.any(AbortSignal) }),
    );
  });

  it("rejects a response that exceeds the byte limit while streaming", async () => {
    const request = vi.fn(() => response(["12345", "67890"])) as PublicTextRequest;

    await expect(
      Effect.runPromise(
        searchWebEffect({ query: "test" }, { request, maxResponseBytes: 8 }),
      ),
    ).rejects.toThrow("Response exceeds the 8-byte limit");
  });

  it("rejects a structurally invalid provider response", async () => {
    const request = vi.fn(() =>
      response([JSON.stringify({ results: [{ title: 42 }], response_time: 1 })]),
    ) as PublicTextRequest;

    await expect(
      Effect.runPromise(searchWebEffect({ query: "test" }, { request })),
    ).rejects.toThrow("Web search failed");
  });
});
