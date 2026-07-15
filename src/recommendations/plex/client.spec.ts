import { describe, expect, it, vi } from "vitest";
import { MediaType } from "../types.js";
import { PlexClient, type PlexGet, parseExternalIds } from "./client.js";

function response(value: Record<string, unknown>) {
  return { MediaContainer: value };
}

describe("PlexClient", () => {
  it("parses modern and legacy external GUIDs", () => {
    expect(
      parseExternalIds({
        guid: "com.plexapp.agents.imdb://tt1234567?lang=en",
        Guid: [{ id: "tmdb://42" }, { id: "tvdb://99" }],
      }),
    ).toEqual({ imdb: "tt1234567", tmdb: 42, tvdb: 99 });
  });

  it("paginates history and aggregates episodes at series level", async () => {
    const get = vi.fn<PlexGet>(async (path, params) => {
      if (path === "/library/metadata/77") {
        return response({
          Metadata: [
            {
              type: "show",
              ratingKey: "77",
              guid: "plex://show/show-id",
              title: "A Show",
              year: 2020,
              leafCount: 10,
              viewedLeafCount: 2,
              Guid: [{ id: "tmdb://700" }, { id: "tvdb://800" }],
            },
          ],
        });
      }
      if (params?.["X-Plex-Container-Start"] === 0) {
        return response({
          totalSize: 2,
          Metadata: [
            {
              type: "episode",
              ratingKey: "701",
              grandparentRatingKey: "77",
              grandparentTitle: "A Show",
              duration: 100,
              viewOffset: 80,
              viewedAt: 1000,
              viewCount: 2,
            },
            {
              type: "episode",
              ratingKey: "702",
              grandparentRatingKey: "77",
              grandparentTitle: "A Show",
              duration: 100,
              viewOffset: 100,
              viewedAt: 2000,
            },
          ],
        });
      }
      throw new Error(`unexpected request ${path}`);
    });

    const result = await new PlexClient(get).fetchWatchHistory();

    expect(result).toEqual([
      {
        guid: "plex://show/show-id",
        title: "A Show",
        year: 2020,
        mediaType: MediaType.Tv,
        externalIds: { tmdb: 700, tvdb: 800 },
        viewedAt: 2_000_000,
        viewCount: 1,
        completion: 0.2,
      },
    ]);
    expect(get).toHaveBeenCalledWith("/library/metadata/77", { includeGuids: 1 });
  });

  it("normalizes movies and keeps only partial continue-watching items", async () => {
    const get: PlexGet = async (path) => {
      expect(path).toBe("/hubs/home/continueWatching");
      return response({
        Metadata: [
          {
            type: "movie",
            guid: "plex://movie/one",
            title: "Movie One",
            year: 2024,
            duration: 1000,
            viewOffset: 250,
            lastViewedAt: 1234,
            Guid: [{ id: "tmdb://12" }, { id: "imdb://tt0000012" }],
          },
          {
            type: "movie",
            guid: "plex://movie/done",
            title: "Done",
            duration: 100,
            viewOffset: 100,
          },
        ],
      });
    };

    await expect(new PlexClient(get).fetchInProgress()).resolves.toEqual([
      {
        guid: "plex://movie/one",
        title: "Movie One",
        year: 2024,
        mediaType: MediaType.Movie,
        externalIds: { tmdb: 12, imdb: "tt0000012" },
        progress: 0.25,
        lastViewedAt: 1_234_000,
      },
    ]);
  });

  it("enriches history movies through bounded metadata detail lookups", async () => {
    const get: PlexGet = async (path) => {
      if (path === "/status/sessions/history/all") {
        return response({
          totalSize: 1,
          Metadata: [
            {
              type: "movie",
              ratingKey: "9",
              guid: "plex://movie/nine",
              title: "Nine",
              viewedAt: 100,
              viewCount: 1,
            },
          ],
        });
      }
      if (path === "/library/metadata/9") {
        return response({
          Metadata: [
            {
              type: "movie",
              ratingKey: "9",
              guid: "plex://movie/nine",
              title: "Nine",
              Guid: [{ id: "tmdb://99" }],
            },
          ],
        });
      }
      throw new Error(`unexpected request ${path}`);
    };

    const history = await new PlexClient(get).fetchWatchHistory();
    expect(history[0]?.externalIds).toEqual({ tmdb: 99 });
  });

  it("loads movie and show sections into one library index", async () => {
    const get: PlexGet = async (path) => {
      if (path === "/library/sections") {
        return response({
          Directory: [
            { key: "1", type: "movie" },
            { key: "2", type: "show" },
            { key: "3", type: "artist" },
          ],
        });
      }
      if (path === "/library/sections/1/all") {
        return response({
          Metadata: [
            {
              type: "movie",
              guid: "plex://movie/a",
              title: "A",
              Guid: [{ id: "tmdb://1" }],
            },
          ],
        });
      }
      if (path === "/library/sections/2/all") {
        return response({
          Metadata: [
            {
              type: "show",
              guid: "plex://show/b",
              title: "B",
              Guid: [{ id: "tvdb://2" }],
            },
          ],
        });
      }
      throw new Error(`unexpected request ${path}`);
    };

    await expect(new PlexClient(get).fetchLibraryIndex()).resolves.toEqual([
      {
        guid: "plex://movie/a",
        title: "A",
        mediaType: MediaType.Movie,
        externalIds: { tmdb: 1 },
        year: undefined,
      },
      {
        guid: "plex://show/b",
        title: "B",
        mediaType: MediaType.Tv,
        externalIds: { tvdb: 2 },
        year: undefined,
      },
    ]);
  });

  it("rejects malformed Plex responses", async () => {
    await expect(new PlexClient(async () => ({})).fetchLibraryIndex()).rejects.toThrow(
      "MediaContainer",
    );
  });

  it("rejects unscoped history containing multiple Plex accounts", async () => {
    const get: PlexGet = async () =>
      response({
        totalSize: 2,
        Metadata: [
          { type: "movie", accountID: 1, guid: "plex://movie/1", title: "One" },
          { type: "movie", accountID: 2, guid: "plex://movie/2", title: "Two" },
        ],
      });
    await expect(new PlexClient(get).fetchWatchHistory()).rejects.toThrow(
      "PLEX_ACCOUNT_ID",
    );
  });

  it("passes the configured Plex account filter to history requests", async () => {
    const get = vi.fn<PlexGet>(async () => response({ totalSize: 0, Metadata: [] }));
    await new PlexClient(get, 7).fetchWatchHistory();
    expect(get).toHaveBeenCalledWith(
      "/status/sessions/history/all",
      expect.objectContaining({ accountID: 7 }),
    );
  });
});
