import { describe, expect, it } from "@effect/vitest";
import { Effect, Exit, Fiber, Result } from "effect";
import { TestClock } from "effect/testing";
import { createHisterService, type HisterFetch } from "./service.js";

const TOKEN = "secret-token";

function response(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

function service(fetchImpl: HisterFetch) {
  return createHisterService("https://hister.test", TOKEN, fetchImpl);
}

describe("Hister HTTP service", () => {
  it.effect(
    "searches with bounded JSON, preserves cursors, and separates history selections",
    () =>
      Effect.gen(function* () {
        const seenUrls: string[] = [];
        const fetchImpl: HisterFetch = (async (input, init) => {
          seenUrls.push(String(input));
          expect(init?.redirect).toBe("error");
          expect(new Headers(init?.headers).get("X-Access-Token")).toBe(TOKEN);
          return response({
            total: 1,
            page_key: "next",
            history: [
              {
                id: "history-1",
                url: "https://example.test/selected",
                title: "Previously selected",
                updated: 1_700_000_000,
              },
            ],
            documents: [
              {
                id: "document-1",
                url: "https://example.test/a",
                title: "A",
                snippet: "snippet",
              },
            ],
          });
        }) as HisterFetch;

        const result = yield* service(fetchImpl).searchEffect({
          query: "term",
          limit: 1,
          cursor: "cursor-in",
          dateFrom: "2026-01-01",
          dateTo: "2026-01-02",
          semantic: true,
        });
        const searchUrl = seenUrls.find((url) => url.includes("/search"));
        const query = JSON.parse(new URL(searchUrl!).searchParams.get("query") ?? "{}");

        expect(query).toMatchObject({
          text: "term",
          limit: 1,
          include_html: false,
          include_text: true,
          semantic_enabled: true,
          page_key: "cursor-in",
          date_from: 1_767_225_600,
          date_to: 1_767_398_399,
        });
        expect(result.total).toBe(1);
        expect(result.nextCursor).toBe("next");
        expect(result.results[0]).toMatchObject({
          documentId: "document-1",
          title: "A",
          url: "https://example.test/a",
          snippet: "snippet",
          titleTruncated: false,
          snippetTruncated: false,
        });
        expect(result.priorSelections).toEqual([
          expect.objectContaining({
            documentId: "history-1",
            title: "Previously selected",
            url: "https://example.test/selected",
            snippet: "",
            updatedAt: 1_700_000_000,
          }),
        ]);
        expect(result.priorSelectionsNote.toLowerCase()).toContain("not constrained");
        expect(result.priorSelectionsNote.toLowerCase()).toContain("matching");
      }),
  );

  it.effect("round-trips the indexed-history cursor", () =>
    Effect.gen(function* () {
      const seenUrls: string[] = [];
      const fetchImpl: HisterFetch = (async (input) => {
        seenUrls.push(String(input));
        return response(
          String(input).includes("last=cursor-1")
            ? {
                documents: [
                  {
                    url: "https://example.test/old",
                    title: "Old",
                    updated: 1_700_000_001,
                  },
                ],
              }
            : {
                page_key: "cursor-1",
                documents: [
                  {
                    url: "https://example.test/new",
                    title: "New",
                    updated: 1_700_000_002,
                  },
                ],
              },
        );
      }) as HisterFetch;
      const hister = service(fetchImpl);

      const first = yield* hister.browseEffect({ filter: "term" });
      const second = yield* hister.browseEffect({
        filter: "term",
        cursor: first.nextCursor,
      });

      expect(first.nextCursor).toBe("cursor-1");
      expect(second.items[0]?.url).toBe("https://example.test/old");
      expect(seenUrls[0]).toContain("filter=term");
      expect(seenUrls[1]).toContain("filter=term");
      expect(seenUrls[1]).toContain("last=cursor-1");
    }),
  );

  it.effect("slices page text and never requests raw HTML", () =>
    Effect.gen(function* () {
      let seenUrl = "";
      const fetchImpl: HisterFetch = (async (input) => {
        seenUrl = String(input);
        return response({
          url: "https://example.test/a",
          title: "A",
          text: "abcdefghij",
        });
      }) as HisterFetch;
      const result = yield* service(fetchImpl).getPageEffect({
        url: "https://example.test/a",
        offset: 2,
        maxChars: 3,
      });

      expect(seenUrl).toContain("/api/document?");
      expect(seenUrl).not.toContain("html");
      expect(result).toEqual({
        title: "A",
        titleTruncated: false,
        url: "https://example.test/a",
        text: "cde",
        totalChars: 10,
        offset: 2,
        nextOffset: 5,
      });
    }),
  );

  it.effect("truncates snippets and titles at their protocol bounds", () =>
    Effect.gen(function* () {
      const title = "t".repeat(1001);
      const snippet = "s".repeat(1501);
      const fetchImpl: HisterFetch = (async () =>
        response({
          total: 1,
          documents: [{ url: "https://example.test/a", title, snippet }],
        })) as HisterFetch;

      const result = yield* service(fetchImpl).searchEffect({ query: "term" });
      expect(result.results[0]?.title).toHaveLength(1000);
      expect(result.results[0]?.titleTruncated).toBe(true);
      expect(result.results[0]?.snippet).toHaveLength(1500);
      expect(result.results[0]?.snippetTruncated).toBe(true);
    }),
  );

  it.effect("rejects URLs longer than the bounded result field", () =>
    Effect.gen(function* () {
      const fetchImpl: HisterFetch = (async () =>
        response({
          total: 1,
          documents: [{ url: "https://example.test/" + "x".repeat(8192), title: "A" }],
        })) as HisterFetch;
      const result = yield* Effect.result(
        service(fetchImpl).searchEffect({ query: "term" }),
      );
      expect(Result.isFailure(result)).toBe(true);
    }),
  );

  it.effect("cancels an oversized response body", () =>
    Effect.gen(function* () {
      let cancelled = false;
      const body = new ReadableStream<Uint8Array>({
        start(controller) {
          controller.enqueue(new Uint8Array(8 * 1024 * 1024 + 1));
        },
        cancel() {
          cancelled = true;
        },
      });
      const fetchImpl: HisterFetch = (async () =>
        new Response(body, { status: 200 })) as HisterFetch;

      const result = yield* Effect.result(
        service(fetchImpl).searchEffect({ query: "term" }),
      );
      expect(Result.isFailure(result)).toBe(true);
      expect(cancelled).toBe(true);
    }),
  );

  it.effect("times out hanging requests on the Effect clock", () =>
    Effect.gen(function* () {
      const fetchImpl: HisterFetch = (async () =>
        new Promise<Response>(() => {})) as HisterFetch;
      const fiber = yield* Effect.forkChild(
        service(fetchImpl).searchEffect({ query: "term" }),
      );
      yield* TestClock.adjust("20 seconds");
      const exit = yield* Fiber.await(fiber);

      expect(Exit.isFailure(exit)).toBe(true);
    }),
  );

  it.effect("aborts an in-flight request when interrupted", () =>
    Effect.gen(function* () {
      let signal: AbortSignal | undefined;
      const fetchImpl: HisterFetch = (async (_input, init) => {
        signal = init?.signal as AbortSignal;
        return new Promise<Response>(() => {});
      }) as HisterFetch;
      const fiber = yield* Effect.forkChild(
        service(fetchImpl).searchEffect({ query: "term" }),
      );
      yield* Effect.yieldNow;
      yield* Fiber.interrupt(fiber);

      expect(signal?.aborted).toBe(true);
    }),
  );

  it.effect("does not leak access tokens or response bodies on HTTP errors", () =>
    Effect.gen(function* () {
      const secretBody = `private-body-${TOKEN}`;
      const fetchImpl: HisterFetch = (async () =>
        response({ secretBody }, 500)) as HisterFetch;
      const exit = yield* Effect.result(
        service(fetchImpl).searchEffect({ query: "term" }),
      );

      expect(Result.isFailure(exit)).toBe(true);
      expect(String(exit)).not.toContain(TOKEN);
      expect(String(exit)).not.toContain(secretBody);
    }),
  );

  it.effect("fails with typed errors for malformed response schemas", () =>
    Effect.gen(function* () {
      const fetchImpl: HisterFetch = (async (input) => {
        const url = String(input);
        if (url.includes("/search")) return response({ total: "bad", documents: [] });
        if (url.includes("/api/document")) return response({ url: 42 });
        return response({ documents: "bad" });
      }) as HisterFetch;
      const hister = service(fetchImpl);

      const search = yield* Effect.result(hister.searchEffect({ query: "term" }));
      const page = yield* Effect.result(
        hister.getPageEffect({ url: "https://example.test/a" }),
      );
      const browse = yield* Effect.result(hister.browseEffect({}));
      expect(Result.isFailure(search)).toBe(true);
      expect(Result.isFailure(page)).toBe(true);
      expect(Result.isFailure(browse)).toBe(true);
    }),
  );

  it.effect(
    "clears labels with one write and does not retry a verification mismatch",
    () =>
      Effect.gen(function* () {
        let clearCalls = 0;
        const clearFetch: HisterFetch = (async (_input, init) => {
          clearCalls += 1;
          return init?.method === "POST"
            ? response({ ok: true })
            : response({ url: "https://example.test/a", label: "" });
        }) as HisterFetch;
        const cleared = yield* service(clearFetch).setLabelEffect({
          url: "https://example.test/a",
          label: "",
        });
        expect(cleared).toEqual({
          url: "https://example.test/a",
          label: "",
          verified: true,
        });
        expect(clearCalls).toBe(2);

        let mismatchCalls = 0;
        const mismatchFetch: HisterFetch = (async (_input, init) => {
          mismatchCalls += 1;
          return init?.method === "POST"
            ? response({ ok: true })
            : response({ url: "https://example.test/a", label: "other" });
        }) as HisterFetch;
        const mismatch = yield* Effect.result(
          service(mismatchFetch).setLabelEffect({
            url: "https://example.test/a",
            label: "wanted",
          }),
        );
        expect(Result.isFailure(mismatch)).toBe(true);
        expect(mismatchCalls).toBe(2);
      }),
  );
});
