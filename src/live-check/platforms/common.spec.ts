import { Effect, Schema } from "effect";
import { describe, expect, it, vi } from "vitest";
import type {
  LimitedTextResponse,
  PublicTextRequest,
} from "../../effect/publicHttp.js";
import { fetchGQL, fetchPageHtml } from "./common.js";

function response(
  chunks: string[],
  contentLength?: number,
  destroy = vi.fn(),
): LimitedTextResponse {
  return {
    response: {
      headers:
        contentLength === undefined ? {} : { "content-length": String(contentLength) },
    },
    destroy,
    async *[Symbol.asyncIterator]() {
      for (const chunk of chunks) yield chunk;
    },
  };
}

describe("bounded platform requests", () => {
  it("rejects a fixed-length oversized page before buffering it", async () => {
    const destroy = vi.fn();
    const request = vi.fn(() => response([], 9, destroy)) as PublicTextRequest;

    await expect(
      Effect.runPromise(
        fetchPageHtml("https://example.com/live", {
          request,
          maxResponseBytes: 8,
        }),
      ),
    ).rejects.toThrow("Response exceeds the 8-byte limit");
    expect(destroy).toHaveBeenCalledOnce();
  });

  it("cancels a chunked oversized GQL response", async () => {
    const destroy = vi.fn();
    const request = vi.fn(() => response(["12345", "67890"], undefined, destroy));

    await expect(
      Effect.runPromise(
        fetchGQL(
          { url: "https://example.com/gql", clientId: "client", query: "{}" },
          Schema.Struct({ ok: Schema.Boolean }),
          { request: request as PublicTextRequest, maxResponseBytes: 8 },
        ),
      ),
    ).rejects.toThrow("Response exceeds the 8-byte limit");
    expect(destroy).toHaveBeenCalledOnce();
  });

  it("decodes a valid GQL response with the concrete schema", async () => {
    const request = vi.fn(() => response(['{"ok":true}']));

    await expect(
      Effect.runPromise(
        fetchGQL(
          { url: "https://example.com/gql", clientId: "client", query: "{}" },
          Schema.Struct({ ok: Schema.Boolean }),
          { request: request as PublicTextRequest },
        ),
      ),
    ).resolves.toEqual({ ok: true });
  });
});
