import { Deferred, Effect, Fiber } from "effect";
import { describe, expect, it, vi } from "vitest";
import { canonicalBindingKey } from "./identityLinks.js";
import { Platform } from "./platforms/index.js";
import {
  bindingFromProfileUrl,
  extractProfileLinks,
  fetchProfileLinksEffect,
  fetchYouTubeVideoOwnerEffect,
  learnProfileIdentityEffect,
  profileIdentityEvidence,
  profilePageUrl,
} from "./profileLinks.js";
import { provideTest } from "./testRuntime.js";

describe("YouTube video ownership", () => {
  const source = { platform: Platform.YouTube, username: "aBcdEFgh_12" };
  const target = { platform: Platform.YouTube, username: "@LonerBoxLive" };
  const metadata = {
    type: "video",
    author_name: "LonerBox Live",
    author_url: "https://www.youtube.com/@lonerboxlive",
  };

  it("learns the configured owner from oEmbed and revalidates only when requested", async () => {
    const fetchImpl = vi.fn(async (_input: string | URL, _init?: RequestInit) =>
      Response.json(metadata),
    );
    const result = await Effect.runPromise(
      Effect.gen(function* () {
        const input = { source, configuredBindings: [target], fetchImpl };
        const first = yield* learnProfileIdentityEffect({ ...input, now: 100 });
        const cached = yield* learnProfileIdentityEffect({ ...input, now: 200 });
        const refreshed = yield* learnProfileIdentityEffect({
          ...input,
          now: 300,
          forceRefresh: true,
        });
        return { first, cached, refreshed };
      }).pipe(provideTest),
    );
    expect(result.first).toEqual({
      sourceBinding: "youtube:aBcdEFgh_12",
      targetBinding: "youtube:@lonerboxlive",
      discoveredAt: 100,
      verifiedAt: 100,
    });
    expect(result.cached).toEqual(result.first);
    expect(result.refreshed).toEqual({ ...result.first, verifiedAt: 300 });
    expect(fetchImpl).toHaveBeenCalledTimes(2);
    const [input, init] = fetchImpl.mock.calls[0]!;
    const url = new URL(input);
    expect(url.origin + url.pathname).toBe("https://www.youtube.com/oembed");
    expect(url.searchParams.get("url")).toBe(
      "https://www.youtube.com/watch?v=aBcdEFgh_12",
    );
    expect(url.searchParams.get("format")).toBe("json");
    expect(init?.redirect).toBe("error");
    expect(new Headers(init?.headers).get("User-Agent")).toBe(
      "OpenAI File Downloader, XaiImageApiFetch/1.0",
    );
  });

  it("returns confirmed no match when a cached owner no longer matches on refresh", async () => {
    const fetchImpl = vi
      .fn(async () => Response.json(metadata))
      .mockImplementationOnce(async () => Response.json(metadata))
      .mockImplementationOnce(async () =>
        Response.json({ ...metadata, author_url: "https://youtube.com/@different" }),
      );
    const result = await Effect.runPromise(
      Effect.gen(function* () {
        const input = { source, configuredBindings: [target], fetchImpl };
        yield* learnProfileIdentityEffect(input);
        return yield* learnProfileIdentityEffect({ ...input, forceRefresh: true });
      }).pipe(provideTest),
    );
    expect(result).toBeUndefined();
    expect(fetchImpl).toHaveBeenCalledTimes(2);
  });

  it.each([
    "https://www.youtube.com/@someoneelse",
    "https://twitch.tv/lonerboxlive",
    "https://example.com/@lonerboxlive",
    "https://www.youtube.com/watch?v=aBcdEFgh_12",
  ])(
    "ignores matching names and unconfigured or non-owner URL %s",
    async (authorUrl) => {
      const fetchImpl = vi.fn(async () =>
        Response.json({ ...metadata, author_url: authorUrl }),
      );
      expect(
        await Effect.runPromise(
          learnProfileIdentityEffect({
            source,
            configuredBindings: [target],
            fetchImpl,
          }).pipe(provideTest),
        ),
      ).toBeUndefined();
      expect(fetchImpl).toHaveBeenCalledOnce();
    },
  );

  it.each([
    () => new Response("unavailable", { status: 500 }),
    () => new Response("not JSON"),
    () => Response.json({ type: "video", author_name: "LonerBox Live" }),
    () => Response.json({ ...metadata, type: "link" }),
  ])("propagates HTTP and metadata failures", async (response) => {
    await expect(
      Effect.runPromise(
        learnProfileIdentityEffect({
          source,
          configuredBindings: [target],
          fetchImpl: async () => response(),
        }).pipe(provideTest),
      ),
    ).rejects.toThrow("fetch YouTube video owner");
  });

  it("bounds owner responses and rejects malformed video identifiers before fetching", async () => {
    const fetchImpl = vi.fn(async () => Response.json(metadata));
    await expect(
      Effect.runPromise(
        fetchYouTubeVideoOwnerEffect(source.username, { fetchImpl, maxBytes: 10 }),
      ),
    ).rejects.toThrow("10 byte limit");
    fetchImpl.mockClear();
    for (const id of ["@lonerboxlive", "abc", "abcdefghij/", "abcdefghijk?url=x"]) {
      expect(
        await Effect.runPromise(fetchYouTubeVideoOwnerEffect(id, { fetchImpl })),
      ).toBeUndefined();
    }
    expect(fetchImpl).not.toHaveBeenCalled();
  });
});

describe("profile link parsing", () => {
  it("extracts direct supported profiles from anchors and structured JSON", () => {
    const html = `
      <a href="https://kick.com/ImReallyImportant?ref=youtube">Kick</a>
      <a href="https://www.youtube.com/redirect?q=https%3A%2F%2Fwww.twitch.tv%2FIRI_live">Twitch</a>
      <script>{"url":"https:\\/\\/www.youtube.com\\/@ImReallyImportant"}</script>
      <a href="https://example.com/links">Elsewhere</a>
    `;

    expect(extractProfileLinks(html).map(canonicalBindingKey).sort()).toEqual([
      "kick:imreallyimportant",
      "twitch:iri_live",
      "youtube:@imreallyimportant",
    ]);
  });

  it("rejects videos, categories, malformed handles, and redirect destinations on other hosts", () => {
    for (const url of [
      "https://youtube.com/watch?v=abc",
      "https://youtube.com/@iri/live",
      "https://kick.com/categories/games",
      "https://twitch.tv/videos/123",
      "https://evil.test/redirect?q=https://kick.com/iri",
      "https://kick.com:444/iri",
      "https://attacker@kick.com/iri",
      "javascript:https://kick.com/iri",
    ]) {
      expect(bindingFromProfileUrl(url), url).toBeUndefined();
    }
  });

  it("builds only canonical, bounded profile-page URLs", () => {
    expect(profilePageUrl({ platform: Platform.YouTube, username: "@IRI" })).toBe(
      "https://www.youtube.com/@iri/about",
    );
    expect(profilePageUrl({ platform: Platform.Kick, username: "IRI" })).toBe(
      "https://kick.com/iri",
    );
    expect(
      profilePageUrl({ platform: Platform.YouTube, username: "channel/UC123" }),
    ).toBe("https://www.youtube.com/channel/UC123/about");
  });
});

describe("profile identity evidence", () => {
  const source = { platform: Platform.Kick, username: "imreallyimportant" };
  const sameHandleTarget = {
    platform: Platform.YouTube,
    username: "@ImReallyImportant",
  };
  const differentTarget = { platform: Platform.YouTube, username: "@IRI" };

  it("accepts a direct configured link when normalized handles match", () => {
    expect(
      profileIdentityEvidence({
        source,
        target: sameHandleTarget,
        directLinks: [sameHandleTarget],
      }),
    ).toBe("equal-handle");
  });

  it("requires a reciprocal link when handles differ", () => {
    expect(
      profileIdentityEvidence({
        source,
        target: differentTarget,
        directLinks: [differentTarget],
      }),
    ).toBeUndefined();
    expect(
      profileIdentityEvidence({
        source,
        target: differentTarget,
        directLinks: [differentTarget],
        reciprocalLinks: [source],
      }),
    ).toBe("reciprocal");
  });

  it("never accepts handle equality without a direct profile link", () => {
    expect(
      profileIdentityEvidence({
        source,
        target: sameHandleTarget,
        directLinks: [],
        reciprocalLinks: [source],
      }),
    ).toBeUndefined();
  });
});

describe("fetchProfileLinks", () => {
  it("uses the repo user agent and parses the bounded response", async () => {
    const calls: Array<{ url: string; init?: RequestInit }> = [];
    const fetchImpl = async (input: string | URL, init?: RequestInit) => {
      calls.push({ url: String(input), init });
      return new Response('<a href="https://youtube.com/@IRI">IRI</a>');
    };

    const result = await Effect.runPromise(
      fetchProfileLinksEffect(
        { platform: Platform.Kick, username: "iri" },
        { fetchImpl },
      ),
    );

    expect(result.map(canonicalBindingKey)).toEqual(["youtube:@iri"]);
    expect(calls[0]?.url).toBe("https://kick.com/iri");
    expect(new Headers(calls[0]?.init?.headers).get("user-agent")).toBe(
      "OpenAI File Downloader, XaiImageApiFetch/1.0",
    );
    expect(calls[0]?.init?.redirect).toBe("error");
  });

  it("stops reading responses beyond the byte cap", async () => {
    const fetchImpl = async () => new Response("x".repeat(20));
    await expect(
      Effect.runPromise(
        fetchProfileLinksEffect(
          { platform: Platform.Kick, username: "iri" },
          { fetchImpl, maxBytes: 10 },
        ),
      ),
    ).rejects.toThrow("10 byte limit");
  });

  it("cancels and releases the response reader when reading fails", async () => {
    const cancel = vi.fn(async () => undefined);
    const releaseLock = vi.fn();
    const reader = {
      read: vi.fn(async () => {
        throw new Error("profile stream failed");
      }),
      cancel,
      releaseLock,
    };
    const fetchImpl = async () =>
      ({
        ok: true,
        status: 200,
        headers: new Headers(),
        body: { getReader: () => reader },
      }) as unknown as Response;

    await expect(
      Effect.runPromise(
        fetchProfileLinksEffect(
          { platform: Platform.Kick, username: "iri" },
          { fetchImpl },
        ),
      ),
    ).rejects.toThrow("profile stream failed");
    expect(cancel).toHaveBeenCalledOnce();
    expect(releaseLock).toHaveBeenCalledOnce();
  });

  it("cancels and releases the response reader when interrupted", async () => {
    const readStarted = await Effect.runPromise(Deferred.make<void>());
    const cancel = vi.fn(async () => undefined);
    const releaseLock = vi.fn();
    const reader = {
      read: vi.fn(() =>
        Effect.runPromise(
          Deferred.succeed(readStarted, undefined).pipe(Effect.andThen(Effect.never)),
        ),
      ),
      cancel,
      releaseLock,
    };
    const fetchImpl = async () =>
      ({
        ok: true,
        status: 200,
        headers: new Headers(),
        body: { getReader: () => reader },
      }) as unknown as Response;

    await Effect.runPromise(
      Effect.gen(function* () {
        const fiber = yield* Effect.forkChild(
          fetchProfileLinksEffect(
            { platform: Platform.Kick, username: "iri" },
            { fetchImpl },
          ),
        );
        yield* Deferred.await(readStarted);
        yield* Fiber.interrupt(fiber);
      }),
    );

    expect(cancel).toHaveBeenCalledOnce();
    expect(releaseLock).toHaveBeenCalledOnce();
  });
});
