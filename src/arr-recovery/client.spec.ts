import { Effect } from "effect";
import { describe, expect, it, vi } from "vitest";
import { HttpArrClient } from "./client.js";
import type { ImportFile, Target } from "./types.js";

const quality = {
  quality: { id: 7, name: "Bluray-1080p", source: "bluray" },
  revision: { version: 1, real: 0, isRepack: false },
  retained: "raw-value",
};

function json(value: unknown, status = 200): Response {
  return Response.json(value, { status });
}

function command(id = 41, status = "queued") {
  return { id, status };
}

function importFile(overrides: Partial<ImportFile> = {}): ImportFile {
  return {
    id: 1,
    path: "/downloads/Show/Show.S01E01.mkv",
    folderName: "Show",
    name: "Show.S01E01",
    size: 1_000,
    seriesId: 9,
    seasonNumber: 1,
    episodeIds: [101],
    quality,
    languages: [{ id: 1, name: "English" }],
    releaseGroup: "GROUP",
    indexerFlags: 0,
    releaseType: "singleEpisode",
    rejections: [],
    ...overrides,
  };
}

function target(overrides: Partial<Target> = {}): Target {
  return {
    id: 9,
    title: "Show",
    year: 2020,
    monitored: true,
    hasFile: false,
    path: "/tv/Show",
    episodeIds: [101],
    episodes: [
      {
        id: 101,
        seasonNumber: 1,
        episodeNumber: 1,
        title: "Pilot",
        hasFile: false,
        monitored: true,
      },
    ],
    alternateTitles: [],
    ...overrides,
  };
}

describe("HttpArrClient", () => {
  it("reads every queue page and sends bounded authenticated requests", async () => {
    const firstRecords = Array.from({ length: 249 }, (_, index) => ({
      id: index + 1,
      downloadId: `download-${index}`,
      title: `Release ${index}`,
      status: "completed",
      trackedDownloadStatus: "warning",
      trackedDownloadState: "importBlocked",
      statusMessages: [{ title: "Import", messages: ["Blocked"] }],
      size: 100,
      sizeleft: 0,
      outputPath: `/downloads/${index}`,
      seriesId: 9,
      episodeId: index + 100,
      protocol: "usenet",
      downloadClient: "NZBGet",
    }));
    firstRecords.push({
      id: 250,
      downloadId: null as unknown as string,
      title: "Pending release",
      status: "delay",
      trackedDownloadStatus: null as unknown as string,
      trackedDownloadState: null as unknown as string,
      statusMessages: null as unknown as { title: string; messages: string[] }[],
      size: 100,
      sizeleft: 100,
      outputPath: null as unknown as string,
      seriesId: 9,
      episodeId: 900,
      protocol: "usenet",
      downloadClient: "NZBGet",
    });
    const fetchImpl = vi.fn(async (input: RequestInfo | URL, _init?: RequestInit) => {
      const page = new URL(String(input)).searchParams.get("page");
      return page === "1"
        ? json({ page: 1, pageSize: 250, totalRecords: 251, records: firstRecords })
        : json({
            page: 2,
            pageSize: 250,
            totalRecords: 251,
            records: [
              {
                id: 251,
                downloadId: "last",
                title: "Last",
                status: "completed",
                size: 1,
                sizeLeft: 0,
              },
            ],
          });
    });
    const client = new HttpArrClient({
      kind: "sonarr",
      url: "http://sonarr.local/base",
      apiKey: "secret",
      fetchImpl,
    });

    const items = await Effect.runPromise(client.queue());

    expect(items).toHaveLength(250);
    expect(items[0]).toMatchObject({
      trackedDownloadState: "importBlocked",
      sizeleft: 0,
      episodeId: 100,
    });
    expect(fetchImpl).toHaveBeenCalledTimes(2);
    const [requestUrl, requestInit] = fetchImpl.mock.calls[0]!;
    expect(String(requestUrl)).toContain("/base/api/v3/queue?page=1&pageSize=250");
    expect(new Headers(requestInit?.headers).get("X-Api-Key")).toBe("secret");
    expect(new Headers(requestInit?.headers).get("User-Agent")).toBe(
      "OpenAI File Downloader, XaiImageApiFetch/1.0",
    );
  });

  it("previews a targeted Sonarr import and retains the full quality object", async () => {
    const fetchImpl = vi.fn(async (_input: RequestInfo | URL, _init?: RequestInit) =>
      json([
        {
          id: 12,
          path: "/downloads/Show/Show.S01E01.mkv",
          folderName: "Show",
          name: "Show.S01E01",
          size: 1_000,
          series: { id: 9, ignored: true },
          seasonNumber: 1,
          episodes: [{ id: 101, title: "Pilot" }],
          quality,
          languages: [{ id: 1, name: "English" }],
          releaseGroup: "GROUP",
          indexerFlags: 0,
          releaseType: "singleEpisode",
          rejections: [],
        },
      ]),
    );
    const client = new HttpArrClient({
      kind: "sonarr",
      url: "http://sonarr.local",
      apiKey: "secret",
      fetchImpl,
    });

    const files = await Effect.runPromise(client.preview("download-id"));

    expect(files).toEqual([
      expect.objectContaining({
        folderName: "Show",
        seriesId: 9,
        episodeIds: [101],
        quality,
      }),
    ]);
    const url = new URL(String(fetchImpl.mock.calls[0]![0]));
    expect(url.searchParams.get("downloadId")).toBe("download-id");
    expect(url.searchParams.has("seriesId")).toBe(false);
    expect(url.searchParams.has("movieId")).toBe(false);
  });

  it("loads every targeted episode in a Sonarr queue group", async () => {
    const fetchImpl = vi.fn(async (input: RequestInfo | URL) => {
      const url = new URL(String(input));
      if (url.pathname.endsWith("/series/9")) {
        return json({
          id: 9,
          title: "Show",
          year: 2020,
          monitored: true,
          path: "/tv/Show",
          alternateTitles: [{ title: "Alias" }],
        });
      }
      return json([
        {
          id: 101,
          seasonNumber: 1,
          episodeNumber: 1,
          title: "One",
          hasFile: true,
          monitored: true,
          episodeFileId: 51,
        },
        {
          id: 102,
          seasonNumber: 1,
          episodeNumber: 2,
          title: "Two",
          hasFile: false,
          monitored: true,
          episodeFileId: 0,
        },
        {
          id: 999,
          seasonNumber: 2,
          episodeNumber: 1,
          title: "Other",
          hasFile: false,
          monitored: true,
        },
      ]);
    });
    const client = new HttpArrClient({
      kind: "sonarr",
      url: "http://sonarr.local",
      apiKey: "secret",
      fetchImpl,
    });

    const result = await Effect.runPromise(
      client.target([
        {
          id: 1,
          downloadId: "pack",
          title: "Pack",
          status: "completed",
          trackedDownloadStatus: "warning",
          trackedDownloadState: "importBlocked",
          statusMessages: [],
          size: 1,
          sizeleft: 0,
          seriesId: 9,
          episodeId: 101,
        },
        {
          id: 2,
          downloadId: "pack",
          title: "Pack",
          status: "completed",
          trackedDownloadStatus: "warning",
          trackedDownloadState: "importBlocked",
          statusMessages: [],
          size: 1,
          sizeleft: 0,
          seriesId: 9,
          episodeId: 102,
        },
      ]),
    );

    expect(result.episodeIds).toEqual([101, 102]);
    expect(result.episodes.map((episode) => episode.id)).toEqual([101, 102]);
    expect(result.hasFile).toBe(false);
    expect(result.alternateTitles).toEqual(["Alias"]);
  });

  it("filters grab history by the exact download id", async () => {
    const fetchImpl = vi.fn(async (_input: RequestInfo | URL, _init?: RequestInit) =>
      json({
        page: 1,
        pageSize: 250,
        totalRecords: 2,
        records: [
          {
            downloadId: "wanted",
            sourceTitle: "Release",
            seriesId: 9,
            episodeId: 101,
            eventType: "grabbed",
            date: "2026-09-12T01:00:00Z",
          },
          {
            downloadId: "other",
            sourceTitle: "Other",
            seriesId: 9,
            episodeId: 102,
            eventType: "grabbed",
            date: "2026-09-12T00:00:00Z",
          },
        ],
      }),
    );
    const client = new HttpArrClient({
      kind: "sonarr",
      url: "http://sonarr.local",
      apiKey: "secret",
      fetchImpl,
    });

    const grabs = await Effect.runPromise(client.history("wanted"));

    expect(grabs).toHaveLength(1);
    const url = new URL(String(fetchImpl.mock.calls[0]![0]));
    expect(url.searchParams.get("downloadId")).toBe("wanted");
    expect(url.searchParams.get("eventType")).toBe("1");
  });

  it("posts the exact manual import and removal controls", async () => {
    const fetchImpl = vi.fn(async (input: RequestInfo | URL, _init?: RequestInit) => {
      const url = new URL(String(input));
      return url.pathname.endsWith("/command") ? json(command()) : new Response(null);
    });
    const client = new HttpArrClient({
      kind: "sonarr",
      url: "http://sonarr.local",
      apiKey: "secret",
      fetchImpl,
    });

    await expect(
      Effect.runPromise(client.importFiles("download-id", [importFile()])),
    ).resolves.toBe(41);
    await Effect.runPromise(client.remove(77, true));

    const importInit = fetchImpl.mock.calls[0]![1];
    expect(JSON.parse(String(importInit?.body))).toEqual({
      name: "ManualImport",
      importMode: "auto",
      files: [
        {
          path: "/downloads/Show/Show.S01E01.mkv",
          folderName: "Show",
          seriesId: 9,
          episodeIds: [101],
          quality,
          languages: [{ id: 1, name: "English" }],
          releaseGroup: "GROUP",
          indexerFlags: 0,
          releaseType: "singleEpisode",
          downloadId: "download-id",
        },
      ],
    });
    const removeUrl = new URL(String(fetchImpl.mock.calls[1]![0]));
    expect(removeUrl.pathname).toBe("/api/v3/queue/77");
    expect(Object.fromEntries(removeUrl.searchParams)).toEqual({
      removeFromClient: "true",
      blocklist: "true",
      skipRedownload: "true",
    });
  });

  it("verifies Sonarr imports by episode file identity and source scene name", async () => {
    const fetchImpl = vi.fn(async (input: RequestInfo | URL) => {
      const path = new URL(String(input)).pathname;
      return path.endsWith("/episode")
        ? json([
            {
              id: 101,
              seasonNumber: 1,
              episodeNumber: 1,
              title: "Pilot",
              hasFile: true,
              monitored: true,
              episodeFileId: 51,
            },
          ])
        : json([
            {
              id: 51,
              path: "/tv/Show/Season 01/01 - Pilot.mkv",
              relativePath: "Season 01/01 - Pilot.mkv",
              sceneName: "Show.S01E01",
            },
          ]);
    });
    const client = new HttpArrClient({
      kind: "sonarr",
      url: "http://sonarr.local",
      apiKey: "secret",
      fetchImpl,
    });

    await expect(
      Effect.runPromise(client.verifyImported(target(), [importFile()])),
    ).resolves.toBe(true);
    await expect(
      Effect.runPromise(
        client.verifyImported(target(), [importFile({ name: "Different.Release" })]),
      ),
    ).resolves.toBe(false);
  });

  it("verifies obfuscated imports against the release folder and exact byte size", async () => {
    let size = 1234;
    const fetchImpl = vi.fn(async (input: RequestInfo | URL) =>
      new URL(String(input)).pathname.endsWith("/episode")
        ? json([
            {
              id: 101,
              seasonNumber: 1,
              episodeNumber: 1,
              title: "Pilot",
              hasFile: true,
              monitored: true,
              episodeFileId: 51,
            },
          ])
        : json([
            {
              id: 51,
              path: "/tv/Show/Season 01/01 - Pilot.mkv",
              relativePath: "Season 01/01 - Pilot.mkv",
              sceneName: "Show.S01E01.1080p-GROUP",
              size,
            },
          ]),
    );
    const client = new HttpArrClient({
      kind: "sonarr",
      url: "http://sonarr.local",
      apiKey: "secret",
      fetchImpl,
    });
    const file = importFile({
      name: "NqFGW2VSR2C49AkyiFgnB6G",
      folderName: "Show.S01E01.1080p-GROUP",
      size: 1234,
    });
    expect(await Effect.runPromise(client.verifyImported(target(), [file]))).toBe(true);
    size = 4321;
    expect(await Effect.runPromise(client.verifyImported(target(), [file]))).toBe(
      false,
    );
  });

  it("ties every Sonarr source to the file id of its own episodes", async () => {
    const fetchImpl = vi.fn(async (input: RequestInfo | URL) => {
      const path = new URL(String(input)).pathname;
      return path.endsWith("/episode")
        ? json([
            {
              id: 101,
              seasonNumber: 1,
              episodeNumber: 1,
              title: "One",
              hasFile: true,
              monitored: true,
              episodeFileId: 52,
            },
            {
              id: 102,
              seasonNumber: 1,
              episodeNumber: 2,
              title: "Two",
              hasFile: true,
              monitored: true,
              episodeFileId: 51,
            },
          ])
        : json([
            {
              id: 51,
              path: "/tv/Show/One.mkv",
              relativePath: "One.mkv",
              sceneName: "One",
            },
            {
              id: 52,
              path: "/tv/Show/Two.mkv",
              relativePath: "Two.mkv",
              sceneName: "Two",
            },
          ]);
    });
    const client = new HttpArrClient({
      kind: "sonarr",
      url: "http://sonarr.local",
      apiKey: "secret",
      fetchImpl,
    });
    const twoEpisodeTarget = target({
      episodeIds: [101, 102],
      episodes: [
        ...target().episodes,
        {
          id: 102,
          seasonNumber: 1,
          episodeNumber: 2,
          title: "Two",
          hasFile: false,
          monitored: true,
        },
      ],
    });

    await expect(
      Effect.runPromise(
        client.verifyImported(twoEpisodeTarget, [
          importFile({ name: "One", episodeIds: [101] }),
          importFile({ id: 2, name: "Two", episodeIds: [102] }),
        ]),
      ),
    ).resolves.toBe(false);
  });

  it("verifies Radarr imports against the original or scene filename", async () => {
    const fetchImpl = vi.fn(async (input: RequestInfo | URL) => {
      const path = new URL(String(input)).pathname;
      return path.endsWith("/movie/4")
        ? json({
            id: 4,
            title: "Movie",
            year: 2024,
            monitored: true,
            path: "/movies/Movie",
            hasFile: true,
            movieFileId: 71,
          })
        : json([
            {
              id: 71,
              movieId: 4,
              path: "/movies/Movie/Movie (2024).mkv",
              relativePath: "Movie (2024).mkv",
              sceneName: "Movie.2024-GROUP",
              originalFilePath: "Movie.2024-GROUP.mkv",
            },
          ]);
    });
    const client = new HttpArrClient({
      kind: "radarr",
      url: "http://radarr.local",
      apiKey: "secret",
      fetchImpl,
    });
    const movieTarget = target({ id: 4, title: "Movie", episodeIds: [], episodes: [] });
    const movieFile = importFile({
      path: "/downloads/Movie.2024-GROUP.mkv",
      name: "Movie.2024-GROUP",
      seriesId: undefined,
      movieId: 4,
      episodeIds: [],
    });

    await expect(
      Effect.runPromise(client.verifyImported(movieTarget, [movieFile])),
    ).resolves.toBe(true);
  });

  it("only confirms removal when the parent listing proves it was readable", async () => {
    const readableFetch = vi.fn(
      async (_input: RequestInfo | URL, _init?: RequestInit) =>
        json({
          directories: [{ path: "/downloads/another" }],
          files: [],
        }),
    );
    const readable = new HttpArrClient({
      kind: "sonarr",
      url: "http://sonarr.local",
      apiKey: "secret",
      fetchImpl: readableFetch,
    });

    await expect(
      Effect.runPromise(readable.verifyRemoved("/downloads/removed")),
    ).resolves.toBe(true);
    const url = new URL(String(readableFetch.mock.calls[0]![0]));
    expect(url.searchParams.get("path")).toBe("/downloads");

    const ambiguous = new HttpArrClient({
      kind: "sonarr",
      url: "http://sonarr.local",
      apiKey: "secret",
      fetchImpl: async () => json({ directories: [], files: [] }),
    });
    await expect(
      Effect.runPromise(ambiguous.verifyRemoved("/downloads/removed")),
    ).resolves.toBe(false);

    const stillPresent = new HttpArrClient({
      kind: "sonarr",
      url: "http://sonarr.local",
      apiKey: "secret",
      fetchImpl: async () =>
        json({ directories: [{ path: "/downloads/removed/" }], files: [] }),
    });
    await expect(
      Effect.runPromise(stillPresent.verifyRemoved("/downloads/removed")),
    ).resolves.toBe(false);
  });

  it("fails with a typed error without exposing the API key", async () => {
    const client = new HttpArrClient({
      kind: "sonarr",
      url: "http://sonarr.local",
      apiKey: "top-secret",
      fetchImpl: async () => json({ error: "no" }, 500),
    });

    const failure = await Effect.runPromise(client.queue().pipe(Effect.flip));

    expect(failure._tag).toBe("ArrRecoveryError");
    expect(failure.message).toContain("HTTP 500");
    expect(failure.message).not.toContain("top-secret");
  });
});
