import { Deferred, Effect, Fiber } from "effect";
import { describe, expect, it, vi } from "vitest";
import { canonicalBindingKey } from "./identityLinks.js";
import { Platform } from "./platforms/index.js";
import {
  bindingFromProfileUrl,
  extractProfileLinks,
  fetchProfileLinksEffect,
  profileIdentityEvidence,
  profilePageUrl,
} from "./profileLinks.js";

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
