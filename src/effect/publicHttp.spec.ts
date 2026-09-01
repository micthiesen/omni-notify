import type { LookupFunction } from "node:net";
import { Effect, Fiber } from "effect";
import { describe, expect, it, vi } from "vitest";
import {
  assertPublicHttpUrlSyntax,
  createPublicDnsLookup,
  fetchPublicText,
  isPublicAddress,
  readBufferResponseWithLimit,
  readFetchResponseBufferWithLimit,
  type LimitedTextResponse,
} from "./publicHttp.js";

function textResponse(
  chunks: Array<string | Uint8Array>,
  contentLength?: number,
): LimitedTextResponse {
  return {
    response: {
      headers:
        contentLength === undefined ? {} : { "content-length": String(contentLength) },
    },
    async *[Symbol.asyncIterator]() {
      for (const chunk of chunks) yield chunk;
    },
  };
}

describe("public HTTP guard", () => {
  it("rejects private, loopback, link-local, and mapped addresses", () => {
    for (const address of [
      "127.0.0.1",
      "10.1.2.3",
      "169.254.169.254",
      "192.168.1.2",
      "::1",
      "fe80::1",
      "::ffff:127.0.0.1",
    ]) {
      expect(isPublicAddress(address), address).toBe(false);
    }
    expect(isPublicAddress("1.1.1.1")).toBe(true);
    expect(isPublicAddress("2606:4700:4700::1111")).toBe(true);
  });

  it("rejects private redirect targets before a connection", () => {
    expect(() => assertPublicHttpUrlSyntax("http://127.0.0.1/admin")).toThrow(
      "public host",
    );
    expect(() =>
      assertPublicHttpUrlSyntax("http://169.254.169.254/latest/meta-data"),
    ).toThrow("public host");
  });

  it.each([
    ["public first", ["1.1.1.1", "127.0.0.1"]],
    ["private first", ["127.0.0.1", "1.1.1.1"]],
  ])(
    "rejects mixed DNS answers with %s for a single lookup",
    async (_label, answers) => {
      const resolve = ((
        _hostname: string,
        options: { all?: boolean },
        callback: (error: null, addresses: unknown[]) => void,
      ) => {
        expect(options.all).toBe(true);
        callback(
          null,
          answers.map((address) => ({ address, family: 4 })),
        );
      }) as unknown as LookupFunction;
      const guarded = createPublicDnsLookup(resolve);
      await expect(
        new Promise((resolveResult, reject) => {
          guarded("redirect.example", { family: 4 }, (error, address) => {
            if (error) reject(error);
            else resolveResult(address);
          });
        }),
      ).rejects.toThrow("public addresses");
    },
  );

  it("validates every answer before returning the requested single shape", async () => {
    const resolve = ((_hostname, options, callback) => {
      expect(options.all).toBe(true);
      callback(null, [
        { address: "1.1.1.1", family: 4 },
        { address: "2606:4700:4700::1111", family: 6 },
      ]);
    }) as LookupFunction;
    const guarded = createPublicDnsLookup(resolve);
    await expect(
      new Promise((resolveResult, reject) => {
        guarded("example.com", {}, (error, address, family) => {
          if (error) reject(error);
          else resolveResult({ address, family });
        });
      }),
    ).resolves.toEqual({ address: "1.1.1.1", family: 4 });
  });

  it("passes AbortSignal to Got and aborts it when interrupted", async () => {
    let receivedSignal: AbortSignal | undefined;
    const request = vi.fn((_url: string | URL, options: { signal?: AbortSignal }) => {
      receivedSignal = options.signal;
      return {
        [Symbol.asyncIterator]() {
          return {
            next: () =>
              new Promise<IteratorResult<string>>((_resolve, reject) => {
                options.signal?.addEventListener("abort", () =>
                  reject(options.signal?.reason),
                );
              }),
          };
        },
      };
    });
    const fiber = Effect.runFork(
      fetchPublicText(
        "https://example.com/feed.xml",
        {},
        "test request",
        request as never,
      ),
    );
    await vi.waitFor(() => expect(receivedSignal).toBeInstanceOf(AbortSignal));
    await Effect.runPromise(Fiber.interrupt(fiber));
    expect(receivedSignal?.aborted).toBe(true);
  });

  it("rejects an oversized fixed-length response before buffering it", async () => {
    const request = vi.fn(() => textResponse(["not buffered"], 6));
    await expect(
      Effect.runPromise(
        fetchPublicText("https://example.com/feed.xml", {}, "test request", request, 5),
      ),
    ).rejects.toThrow("Response exceeds the 5-byte limit");
  });

  it("rejects an oversized declared empty body before reading and closes it", async () => {
    const next = vi.fn(async () => ({ done: true, value: undefined }) as const);
    const close = vi.fn(async () => ({ done: true, value: undefined }) as const);
    const request = vi.fn((): LimitedTextResponse => ({
      response: { headers: { "content-length": "6" } },
      [Symbol.asyncIterator]() {
        return { next, return: close };
      },
    }));

    await expect(
      Effect.runPromise(
        fetchPublicText("https://example.com/empty", {}, "test request", request, 5),
      ),
    ).rejects.toThrow("Response exceeds the 5-byte limit");
    expect(next).not.toHaveBeenCalled();
    expect(close).toHaveBeenCalledOnce();
  });

  it("checks late response headers even when the stream has no chunks", async () => {
    const close = vi.fn(async () => ({ done: true, value: undefined }) as const);
    const headers: Record<string, string> = {};
    const response: LimitedTextResponse = {
      response: { headers },
      [Symbol.asyncIterator]() {
        return {
          next: async () => {
            headers["content-length"] = "6";
            return { done: true, value: undefined } as const;
          },
          return: close,
        };
      },
    };

    await expect(
      Effect.runPromise(
        fetchPublicText(
          "https://example.com/empty",
          {},
          "test request",
          () => response,
          5,
        ),
      ),
    ).rejects.toThrow("Response exceeds the 5-byte limit");
    expect(close).toHaveBeenCalledOnce();
  });

  it("rejects an oversized chunked response while streaming", async () => {
    const request = vi.fn(() => textResponse(["123", "456"]));
    await expect(
      Effect.runPromise(
        fetchPublicText("https://example.com/feed.xml", {}, "test request", request, 5),
      ),
    ).rejects.toThrow("Response exceeds the 5-byte limit");
  });

  it("rejects fixed-length binary responses before reading and cancels them", async () => {
    const cancel = vi.fn();
    const response = new Response(
      new ReadableStream<Uint8Array>({
        pull(controller) {
          controller.enqueue(new Uint8Array([1, 2, 3]));
        },
        cancel,
      }),
      { headers: { "content-length": "6" } },
    );
    await expect(readFetchResponseBufferWithLimit(response, 5)).rejects.toThrow(
      "Response exceeds the 5-byte limit",
    );
    expect(cancel).toHaveBeenCalledOnce();
  });

  it("observes a rejected stream cancellation when the read is aborted", async () => {
    const cancellationError = new Error("cancel failed");
    const cancel = vi.fn(() => Promise.reject(cancellationError));
    const response = new Response(
      new ReadableStream<Uint8Array>({
        cancel,
      }),
    );
    const controller = new AbortController();
    const unhandled: unknown[] = [];
    const onUnhandled = (reason: unknown) => unhandled.push(reason);
    process.on("unhandledRejection", onUnhandled);

    try {
      const reading = readFetchResponseBufferWithLimit(response, 5, controller.signal);
      const interruption = new Error("interrupted");
      controller.abort(interruption);
      await expect(reading).rejects.toBe(interruption);
      await new Promise<void>((resolve) => setImmediate(resolve));
      expect(cancel).toHaveBeenCalledOnce();
      expect(unhandled).toEqual([]);
    } finally {
      process.off("unhandledRejection", onUnhandled);
    }
  });

  it("rejects chunked binary responses on overflow and destroys the source", async () => {
    const destroy = vi.fn();
    const response: LimitedTextResponse = {
      destroy,
      async *[Symbol.asyncIterator]() {
        yield new Uint8Array([1, 2, 3]);
        yield new Uint8Array([4, 5, 6]);
      },
    };
    await expect(readBufferResponseWithLimit(response, 5)).rejects.toThrow(
      "Response exceeds the 5-byte limit",
    );
    expect(destroy).toHaveBeenCalledOnce();
  });
});
