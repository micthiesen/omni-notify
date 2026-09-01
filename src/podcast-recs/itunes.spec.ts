import { Effect } from "effect";
import { describe, expect, it, vi } from "vitest";
import type { LimitedTextResponse, PublicTextRequest } from "../effect/publicHttp.js";
import {
  type ItunesShow,
  pickBestShowMatch,
  searchItunesPodcastsEffect,
} from "./itunes.js";

function response(chunks: string[]): LimitedTextResponse {
  return {
    async *[Symbol.asyncIterator]() {
      for (const chunk of chunks) yield chunk;
    },
  };
}

function show(overrides: Partial<ItunesShow> & { itunesId: number }): ItunesShow {
  return { title: "Untitled", genres: [], ...overrides };
}

describe("pickBestShowMatch", () => {
  it("picks the exact normalized match", () => {
    const shows = [
      show({ itunesId: 1, title: "The Daily" }),
      show({ itunesId: 2, title: "Reply All" }),
    ];
    expect(pickBestShowMatch(shows, "reply all")).toEqual(shows[1]);
  });

  it("matches despite punctuation and casing differences", () => {
    const shows = [show({ itunesId: 1, title: "Radio Lab" })];
    expect(pickBestShowMatch(shows, "radio-lab!!")).toEqual(shows[0]);
  });

  it("matches despite diacritics", () => {
    const shows = [show({ itunesId: 1, title: "Café Society" })];
    expect(pickBestShowMatch(shows, "cafe society")).toEqual(shows[0]);
  });

  it("falls back to containment when the query is a prefix of the title", () => {
    const shows = [show({ itunesId: 1, title: "Reply All: The Podcast" })];
    expect(pickBestShowMatch(shows, "Reply All")).toEqual(shows[0]);
  });

  it("falls back to containment when the title is a prefix of the query", () => {
    const shows = [show({ itunesId: 1, title: "Reply All" })];
    expect(pickBestShowMatch(shows, "Reply All: The Podcast")).toEqual(shows[0]);
  });

  it("returns undefined when nothing matches", () => {
    const shows = [show({ itunesId: 1, title: "The Daily" })];
    expect(pickBestShowMatch(shows, "Completely Unrelated Show")).toBeUndefined();
  });

  it("returns undefined for an empty shows list", () => {
    expect(pickBestShowMatch([], "Anything")).toBeUndefined();
  });
});

describe("searchItunesPodcastsEffect", () => {
  it("streams and decodes the bounded iTunes response", async () => {
    const request = vi.fn(() =>
      response([
        JSON.stringify({
          results: [
            {
              collectionId: 123,
              collectionName: "Example Podcast",
              feedUrl: "https://example.com/feed.xml",
              genres: ["News"],
            },
          ],
        }),
      ]),
    ) as PublicTextRequest;

    await expect(
      Effect.runPromise(
        searchItunesPodcastsEffect("example", 5, {
          request,
          maxResponseBytes: 1024,
        }),
      ),
    ).resolves.toEqual([
      {
        itunesId: 123,
        title: "Example Podcast",
        feedUrl: "https://example.com/feed.xml",
        artworkUrl: undefined,
        genres: ["News"],
      },
    ]);
    expect(request).toHaveBeenCalledWith(
      "https://itunes.apple.com/search",
      expect.objectContaining({ signal: expect.any(AbortSignal) }),
    );
  });

  it("rejects a response that exceeds the byte limit while streaming", async () => {
    const request = vi.fn(() => response(["12345", "67890"])) as PublicTextRequest;

    await expect(
      Effect.runPromise(
        searchItunesPodcastsEffect("example", 5, {
          request,
          maxResponseBytes: 8,
        }),
      ),
    ).rejects.toThrow("Response exceeds the 8-byte limit");
  });

  it("rejects a structurally invalid provider response", async () => {
    const request = vi.fn(() =>
      response([JSON.stringify({ results: [{ collectionId: "123" }] })]),
    ) as PublicTextRequest;

    await expect(
      Effect.runPromise(searchItunesPodcastsEffect("example", 5, { request })),
    ).rejects.toThrow();
  });
});
