import { afterEach, describe, expect, it, vi } from "vitest";
import { Effect, Fiber } from "effect";
import {
  assertTrustedCaldavUrl,
  CALDAV_REQUEST_TIMEOUT_MS,
  CALDAV_XML_MAX_BYTES,
  propfindEffect,
} from "./http.js";

const propfind = (...args: Parameters<typeof propfindEffect>) =>
  Effect.runPromise(propfindEffect(...args));

function abortableFetchMock(): {
  fetchMock: ReturnType<typeof vi.fn>;
  started: Promise<AbortSignal>;
} {
  const started = Promise.withResolvers<AbortSignal>();
  const fetchMock = vi.fn(
    (_input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
      const signal = init?.signal as AbortSignal;
      started.resolve(signal);
      return new Promise((_resolve, reject) => {
        signal.addEventListener("abort", () => reject(signal.reason), { once: true });
      });
    },
  );
  return { fetchMock, started: started.promise };
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("CalDAV HTTP safety", () => {
  it("aborts an in-flight PROPFIND when its Effect is interrupted", async () => {
    const { fetchMock, started } = abortableFetchMock();
    vi.stubGlobal("fetch", fetchMock);

    const fiber = Effect.runFork(
      propfindEffect("https://caldav.icloud.com/", "Basic secret", "0", "<propfind/>"),
    );
    const signal = await started;
    expect(signal.aborted).toBe(false);

    await Effect.runPromise(Fiber.interrupt(fiber));

    expect(signal.aborted).toBe(true);
    expect(fetchMock).toHaveBeenCalledOnce();
  });

  it("follows iCloud shard redirects with authorization and a timeout", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(
        new Response(null, {
          status: 302,
          headers: { location: "https://p42-caldav.icloud.com/123/" },
        }),
      )
      .mockResolvedValueOnce(new Response("<multistatus/>", { status: 207 }));
    vi.stubGlobal("fetch", fetchMock);

    const result = await propfind(
      "https://caldav.icloud.com/",
      "Basic secret",
      "0",
      "<propfind/>",
    );

    expect(result.url).toBe("https://p42-caldav.icloud.com/123/");
    expect(fetchMock).toHaveBeenCalledTimes(2);
    for (const [, init] of fetchMock.mock.calls) {
      expect(init.headers.Authorization).toBe("Basic secret");
      expect(init.signal).toBeInstanceOf(AbortSignal);
    }
    expect(CALDAV_REQUEST_TIMEOUT_MS).toBe(15_000);
  });

  it("refuses to forward authorization to an untrusted redirect host", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(null, {
        status: 302,
        headers: { location: "https://attacker.example/collect" },
      }),
    );
    vi.stubGlobal("fetch", fetchMock);

    await expect(
      propfind("https://caldav.icloud.com/", "Basic secret", "0", "<propfind/>"),
    ).rejects.toThrow("Refusing to forward CalDAV credentials");
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it("accepts only iCloud HTTPS collection URLs", () => {
    expect(
      assertTrustedCaldavUrl("https://p03-caldav.icloud.com/123/calendars/home/"),
    ).toContain("p03-caldav.icloud.com");
    expect(() => assertTrustedCaldavUrl("http://caldav.icloud.com/")).toThrow(
      "Untrusted iCloud CalDAV URL",
    );
    expect(() =>
      assertTrustedCaldavUrl("https://icloud.com.attacker.example/"),
    ).toThrow("Untrusted iCloud CalDAV URL");
  });

  it("rejects a response whose Content-Length exceeds the XML limit", async () => {
    const cancel = vi.fn();
    const body = new ReadableStream<Uint8Array>({
      pull() {},
      cancel,
    });
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(
        new Response(body, {
          status: 207,
          headers: { "Content-Length": String(CALDAV_XML_MAX_BYTES + 1) },
        }),
      ),
    );

    await expect(
      propfind("https://caldav.icloud.com/", "Basic secret", "0", "<propfind/>"),
    ).rejects.toThrow(`response exceeds the ${CALDAV_XML_MAX_BYTES}-byte limit`);
    expect(cancel).toHaveBeenCalledOnce();
  });

  it("cancels a chunked response as soon as it exceeds the XML limit", async () => {
    const cancel = vi.fn();
    let pulls = 0;
    const body = new ReadableStream<Uint8Array>({
      pull(controller) {
        pulls += 1;
        controller.enqueue(new Uint8Array(1024 * 1024));
      },
      cancel,
    });
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(new Response(body, { status: 207 })),
    );

    await expect(
      propfind("https://caldav.icloud.com/", "Basic secret", "0", "<propfind/>"),
    ).rejects.toThrow(`response exceeds the ${CALDAV_XML_MAX_BYTES}-byte limit`);
    expect(cancel).toHaveBeenCalledOnce();
    expect(pulls).toBeLessThanOrEqual(4);
  });
});
