import { Effect } from "effect";
import { describe, expect, it, vi } from "vitest";
import type { LimitedTextResponse } from "../effect/publicHttp.js";
import {
  assertPublicHttpUrlSyntax,
  createPublicDnsLookup,
  fetchPublicBuffer,
  fetchPublicHtml,
  fetchPublicJson,
  isPublicAddress,
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

describe("PressPods public HTTP guard", () => {
  it.each([
    "127.0.0.1",
    "10.0.0.1",
    "100.64.0.1",
    "169.254.169.254",
    "172.16.0.1",
    "192.168.1.1",
    "224.0.0.1",
    "::1",
    "fc00::1",
    "fe80::1",
    "::ffff:127.0.0.1",
    "::ffff:7f00:1",
    "64:ff9b::7f00:1",
  ])("rejects non-public address %s", (address) => {
    expect(isPublicAddress(address)).toBe(false);
  });

  it.each(["1.1.1.1", "8.8.8.8", "2606:4700:4700::1111"])(
    "allows public address %s",
    (address) => {
      expect(isPublicAddress(address)).toBe(true);
    },
  );

  it.each([
    "file:///etc/passwd",
    "ftp://example.com/article",
    "http://localhost/article",
    "http://service.localhost/article",
    "http://127.0.0.1/article",
    "http://[::1]/article",
    "https://user:password@example.com/article",
  ])("rejects unsafe URL %s before any request", (url) => {
    expect(() => assertPublicHttpUrlSyntax(url)).toThrow();
  });

  it("allows a public HTTP URL", () => {
    expect(assertPublicHttpUrlSyntax("https://example.com/article").href).toBe(
      "https://example.com/article",
    );
  });

  it.each([
    ["public first", ["1.1.1.1", "127.0.0.1"]],
    ["private first", ["127.0.0.1", "1.1.1.1"]],
  ])(
    "rejects mixed DNS answers with %s for a single lookup",
    async (_label, answers) => {
      const resolver = ((_hostname, options, callback) => {
        expect(options.all).toBe(true);
        callback(
          null,
          answers.map((address) => ({ address, family: 4 })),
        );
      }) as Parameters<typeof createPublicDnsLookup>[0];
      const guardedLookup = createPublicDnsLookup(resolver);

      await expect(
        new Promise<void>((resolve, reject) => {
          guardedLookup("redirect.example", { family: 4 }, (error) => {
            if (error) reject(error);
            else resolve();
          });
        }),
      ).rejects.toThrow("resolve only to public addresses");
    },
  );

  it("validates every answer before returning the requested single shape", async () => {
    const resolver = ((_hostname, options, callback) => {
      expect(options.all).toBe(true);
      callback(null, [
        { address: "1.1.1.1", family: 4 },
        { address: "2606:4700:4700::1111", family: 6 },
      ]);
    }) as Parameters<typeof createPublicDnsLookup>[0];
    const guardedLookup = createPublicDnsLookup(resolver);

    await expect(
      new Promise((resolve, reject) => {
        guardedLookup("example.com", {}, (error, address, family) => {
          if (error) reject(error);
          else resolve({ address, family });
        });
      }),
    ).resolves.toEqual({ address: "1.1.1.1", family: 4 });
  });

  it("preserves the all-address lookup shape used by Node 24", async () => {
    const resolver = ((_hostname, options, callback) => {
      expect(options.all).toBe(true);
      callback(null, [
        { address: "1.1.1.1", family: 4 },
        { address: "2606:4700:4700::1111", family: 6 },
      ]);
    }) as Parameters<typeof createPublicDnsLookup>[0];
    const guardedLookup = createPublicDnsLookup(resolver);

    await expect(
      new Promise((resolve, reject) => {
        guardedLookup("example.com", { all: true }, (error, addresses) => {
          if (error) reject(error);
          else resolve(addresses);
        });
      }),
    ).resolves.toEqual([
      { address: "1.1.1.1", family: 4 },
      { address: "2606:4700:4700::1111", family: 6 },
    ]);
  });

  it("rejects an all-address lookup containing any private address", async () => {
    const resolver = ((_hostname, _options, callback) => {
      callback(null, [
        { address: "1.1.1.1", family: 4 },
        { address: "127.0.0.1", family: 4 },
      ]);
    }) as Parameters<typeof createPublicDnsLookup>[0];
    const guardedLookup = createPublicDnsLookup(resolver);

    await expect(
      new Promise<void>((resolve, reject) => {
        guardedLookup("example.com", { all: true }, (error) => {
          if (error) reject(error);
          else resolve();
        });
      }),
    ).rejects.toThrow("resolve only to public addresses");
  });

  it("rejects an oversized fixed-length HTML response", async () => {
    const request = vi.fn(() => textResponse(["not buffered"], 6));
    await expect(
      Effect.runPromise(
        fetchPublicHtml("https://example.com/article", "test-agent", request, 5),
      ),
    ).rejects.toThrow("Response exceeds the 5-byte limit");
  });

  it("rejects an oversized chunked HTML response while streaming", async () => {
    const request = vi.fn(() => textResponse(["123", "456"]));
    await expect(
      Effect.runPromise(
        fetchPublicHtml("https://example.com/article", "test-agent", request, 5),
      ),
    ).rejects.toThrow("Response exceeds the 5-byte limit");
  });

  it("bounds fixed-length JSON before parsing", async () => {
    const request = vi.fn(() => textResponse(['{"ok":true}'], 11));
    await expect(
      Effect.runPromise(
        fetchPublicJson("https://example.com/data", {}, "test JSON", 10, request),
      ),
    ).rejects.toThrow("Response exceeds the 10-byte limit");
  });

  it("bounds chunked binary downloads and never returns a partial buffer", async () => {
    const request = vi.fn(() =>
      textResponse([new Uint8Array([1, 2, 3]), new Uint8Array([4, 5, 6])]),
    );
    await expect(
      Effect.runPromise(
        fetchPublicBuffer("https://example.com/image", {}, "test image", 5, request),
      ),
    ).rejects.toThrow("Response exceeds the 5-byte limit");
  });
});
