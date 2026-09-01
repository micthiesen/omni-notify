import { Effect, Fiber } from "effect";
import type { OptionsInit } from "got";
import { describe, expect, it, vi } from "vitest";
import { CastroApi, type CastroHttpRequest } from "./api.js";

function api(request: CastroHttpRequest, maxResponseBytes = 1024): CastroApi {
  return new CastroApi(
    {
      accessId: "device",
      secret: new TextEncoder().encode("secret"),
    },
    { request, maxResponseBytes },
  );
}

function response(chunks: string[], contentLength?: number, destroy = vi.fn()) {
  return {
    response: {
      statusCode: 200,
      headers:
        contentLength === undefined ? {} : { "content-length": String(contentLength) },
    },
    destroy,
    async *[Symbol.asyncIterator]() {
      for (const chunk of chunks) yield chunk;
    },
  };
}

describe("CastroApi bounded requests", () => {
  it("streams and decodes a concrete response", async () => {
    const request = vi.fn(() =>
      response([
        JSON.stringify({
          device_status: 1,
          account_status: 1,
          latest_event_id: 42,
        }),
      ]),
    ) as CastroHttpRequest;

    await expect(Effect.runPromise(api(request).getSyncStatus())).resolves.toEqual({
      device_status: 1,
      account_status: 1,
      latest_event_id: 42,
    });
    expect(request).toHaveBeenCalledWith(
      "https://tentacles.castro.fm/profile/sync/status",
      expect.objectContaining({ signal: expect.any(AbortSignal) }),
    );
  });

  it("rejects a fixed-length oversized response before buffering it", async () => {
    const destroy = vi.fn();
    const request = vi.fn(() => response([], 9, destroy)) as CastroHttpRequest;

    await expect(Effect.runPromise(api(request, 8).getSyncStatus())).rejects.toThrow(
      "Response exceeds the 8-byte limit",
    );
    expect(destroy).toHaveBeenCalledTimes(3);
  });

  it("cancels a chunked oversized response", async () => {
    const destroy = vi.fn();
    const request = vi.fn(() =>
      response(["12345", "67890"], undefined, destroy),
    ) as CastroHttpRequest;

    await expect(Effect.runPromise(api(request, 8).getSyncStatus())).rejects.toThrow(
      "Response exceeds the 8-byte limit",
    );
    expect(destroy).toHaveBeenCalledTimes(3);
  });
});

describe("CastroApi interruption", () => {
  it("aborts an in-flight request when its Effect is interrupted", async () => {
    const signals: AbortSignal[] = [];
    const request = vi.fn((_url: string | URL, options: OptionsInit) => ({
      async *[Symbol.asyncIterator]() {
        yield* [];
        const signal = options.signal as AbortSignal;
        signals.push(signal);
        await Effect.runPromise(
          Effect.callback<never, DOMException>((resume) => {
            const onAbort = () =>
              resume(Effect.fail(new DOMException("Aborted", "AbortError")));
            signal.addEventListener("abort", onAbort, { once: true });
            return Effect.sync(() => signal.removeEventListener("abort", onAbort));
          }),
        );
      },
    })) as CastroHttpRequest;
    const fiber = Effect.runFork(api(request).getSyncStatus());

    await vi.waitFor(() => expect(signals).toHaveLength(1));
    await Effect.runPromise(Fiber.interrupt(fiber));

    expect(signals[0]?.aborted).toBe(true);
  });
});
