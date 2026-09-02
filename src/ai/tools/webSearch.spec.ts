import type { Docstore } from "@micthiesen/mitools/docstore";
import type { Logger } from "@micthiesen/mitools/logging";
import { Effect } from "effect";
import { afterAll, describe, expect, it, vi } from "vitest";
import type {
  LimitedTextResponse,
  PublicTextRequest,
} from "../../effect/publicHttp.js";
import { runnerFromContext } from "../../effect/appRuntime.js";
import { getCostEvents } from "../../costs/persistence.js";
import {
  runWithLogCaptureEffect,
  startRunLogCapture,
  takeRunLogCapture,
} from "../../task-runs/logCapture.js";
import { createMitoolsTestRuntime } from "../../test/mitools.js";
import { makeWebSearchTool, searchWebEffect } from "./webSearch.js";

const runtime = createMitoolsTestRuntime();
afterAll(() => runtime.dispose());

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
      runtime.run(
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
      runtime.run(searchWebEffect({ query: "test" }, { request, maxResponseBytes: 8 })),
    ).rejects.toThrow("Response exceeds the 8-byte limit");
  });

  it("rejects a structurally invalid provider response", async () => {
    const request = vi.fn(() =>
      response([JSON.stringify({ results: [{ title: 42 }], response_time: 1 })]),
    ) as PublicTextRequest;

    await expect(
      runtime.run(searchWebEffect({ query: "test" }, { request })),
    ).rejects.toThrow("Web search failed");
  });

  it("retains task attribution when the AI SDK invokes the captured tool later", async () => {
    const runId = "web-search-tool-context";
    startRunLogCapture(runId, "Recommendations");
    const request = vi.fn(() =>
      response([
        JSON.stringify({
          results: [],
          response_time: 0.1,
        }),
      ]),
    ) as PublicTextRequest;
    try {
      const searchTool = await runtime.run(
        runWithLogCaptureEffect(
          runId,
          Effect.gen(function* () {
            const context = yield* Effect.context<Logger | Docstore>();
            return makeWebSearchTool(runnerFromContext(context), { request });
          }),
        ),
      );

      await searchTool.execute?.(
        { query: "context test" },
        { toolCallId: "tool-call", messages: [], context: {} },
      );

      const events = await runtime.run(getCostEvents());
      expect(events).toContainEqual(
        expect.objectContaining({
          runId,
          feature: "media-recommendations",
          service: "tavily",
        }),
      );
    } finally {
      takeRunLogCapture(runId);
    }
  });
});
