import { Effect } from "effect";
import { describe, expect, it } from "vitest";
import { ArrRepairClient, type ArrInspection } from "./arr.js";

const episode = (overrides: Record<string, unknown> = {}) => ({
  id: 10,
  seriesId: 7,
  seasonNumber: 2,
  episodeNumber: 14,
  hasFile: true,
  monitored: true,
  episodeFileId: 20,
  ...overrides,
});
const history = (overrides: Record<string, unknown> = {}) => ({
  id: 30,
  seriesId: 7,
  episodeId: 10,
  sourceTitle: "Show.S02E14.1080p-GRP",
  downloadId: "download-1",
  eventType: "downloadFolderImported",
  data: { fileId: "20" },
  ...overrides,
});
function inspected(overrides: Partial<ArrInspection> = {}): ArrInspection {
  return {
    kind: "sonarr",
    id: 7,
    title: "Show",
    monitored: true,
    episodes: [episode()] as ArrInspection["episodes"],
    files: [
      {
        id: 20,
        seriesId: 7,
        path: "/tv/Show/S02E14.mkv",
        sceneName: "Show.S02E14.1080p-GRP",
      },
    ],
    history: [
      history(),
      history({ id: 31, eventType: "grabbed" }),
    ] as ArrInspection["history"],
    queue: [],
    ...overrides,
  };
}
function client(fetchImpl: typeof fetch = async () => new Response("{}")) {
  return new ArrRepairClient({
    kind: "sonarr",
    url: "http://sonarr.local",
    apiKey: "key",
    fetchImpl,
  });
}

describe("ArrRepairClient planning", () => {
  it("plans an exact imported file and matching grabbed release", async () => {
    const result = await Effect.runPromise(
      client().plan(inspected(), { action: "replace", season: 2, episodes: [14] }),
    );
    expect(result).toMatchObject({
      episodeIds: [10],
      fileIds: [20],
      historyIds: [31],
      releases: ["Show.S02E14.1080p-GRP"],
    });
  });
  it("rejects a shared file containing episodes outside the requested scope", async () => {
    await expect(
      Effect.runPromise(
        client().plan(
          inspected({
            episodes: [
              episode(),
              episode({ id: 11, episodeNumber: 15, episodeFileId: 20 }),
            ] as ArrInspection["episodes"],
          }),
          { action: "replace", season: 2, episodes: [14] },
        ),
      ),
    ).rejects.toThrow("outside the requested scope");
  });
  it("rejects a season release whose grab covers episodes outside the requested scope", async () => {
    await expect(
      Effect.runPromise(
        client().plan(
          inspected({
            history: [
              history(),
              history({ id: 31, episodeId: 11, eventType: "grabbed" }),
            ] as ArrInspection["history"],
          }),
          { action: "replace", season: 2, episodes: [14] },
        ),
      ),
    ).rejects.toThrow("outside scope");
  });
  it("searches only missing episodes while preserving present episodes", async () => {
    const result = await Effect.runPromise(
      client().plan(
        inspected({
          episodes: [
            episode(),
            episode({ id: 11, episodeNumber: 15, hasFile: false, episodeFileId: 0 }),
          ] as ArrInspection["episodes"],
        }),
        { action: "search_missing", season: 2, episodes: [] },
      ),
    );
    expect(result.episodeIds).toEqual([11]);
    expect(result.fileIds).toEqual([]);
  });
  it("rejects nonexistent specific episodes", async () => {
    await expect(
      Effect.runPromise(
        client().plan(inspected(), {
          action: "search_missing",
          season: 2,
          episodes: [99],
        }),
      ),
    ).rejects.toBeDefined();
  });
});

describe("ArrRepairClient inspection and mutation", () => {
  it("uses the series endpoint when both external IDs are present", async () => {
    const urls: string[] = [];
    const fetchImpl: typeof fetch = async (input) => {
      const url = String(input);
      urls.push(url);
      if (url.includes("/api/v3/series?"))
        return new Response(
          JSON.stringify([
            { id: 7, title: "Show", tvdbId: 123, tmdbId: 456, monitored: true },
          ]),
        );
      if (url.includes("/episode?")) return new Response(JSON.stringify([episode()]));
      if (url.includes("/episodefile?"))
        return new Response(
          JSON.stringify([
            { id: 20, seriesId: 7, path: "/x", sceneName: "Show.S02E14" },
          ]),
        );
      if (url.includes("/history/series?"))
        return new Response(JSON.stringify([history()]));
      if (url.includes("/queue?"))
        return new Response(JSON.stringify({ totalRecords: 0, records: [] }));
      return new Response("[]");
    };
    await expect(
      Effect.runPromise(
        new ArrRepairClient({
          kind: "sonarr",
          url: "http://sonarr.local",
          apiKey: "key",
          fetchImpl,
        }).inspect({ tmdbId: 456, tvdbId: 123, mediaType: "tv" }),
      ),
    ).resolves.toBeDefined();
    expect(urls[0]).toContain("/api/v3/series?tvdbId=123");
  });
  it("blocklists, verifies, deletes, verifies, then starts search", async () => {
    const calls: string[] = [];
    const fetchImpl: typeof fetch = async (input) => {
      const url = String(input);
      calls.push(url);
      if (url.endsWith("/command")) return new Response(JSON.stringify({ id: 99 }));
      if (url.includes("blocklist?"))
        return new Response(
          JSON.stringify({
            records: [{ sourceTitle: "Show.S02E14.1080p-GRP", seriesId: 7 }],
          }),
        );
      if (url.includes("episodefile?")) return new Response(JSON.stringify([]));
      return new Response("");
    };
    await expect(
      Effect.runPromise(
        client(fetchImpl).replace({
          kind: "sonarr",
          id: 7,
          title: "Show",
          description: "",
          episodeIds: [10],
          fileIds: [20],
          historyIds: [30],
          releases: ["Show.S02E14.1080p-GRP"],
        }),
      ),
    ).resolves.toBe(99);
    expect(calls.map((url) => url.split("/api/v3/")[1]?.split("?")[0])).toEqual([
      "history/failed/30",
      "blocklist",
      "episodefile/20",
      "episodefile",
      "command",
    ]);
  });
});
