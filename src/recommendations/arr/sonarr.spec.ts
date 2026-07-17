import { describe, expect, it, vi } from "vitest";
import type { ArrConfig } from "./client.js";
import { addSonarrSeries, fetchSonarrSeries } from "./sonarr.js";

const config: ArrConfig = {
  url: "http://sonarr:8989",
  apiKey: "sonarr-key",
  rootFolderPath: "/tv",
  qualityProfileId: 7,
};

function json(value: unknown, status = 200): Response {
  return new Response(JSON.stringify(value), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

describe("Sonarr adapter", () => {
  it("normalizes tracked series with TVDB and available TMDB ids", async () => {
    const fetchMock = vi.fn<typeof fetch>().mockResolvedValue(
      json([
        {
          id: 12,
          title: "Severance",
          year: 2022,
          tvdbId: 371980,
          tmdbId: 95396,
          imdbId: "tt11280740",
        },
      ]),
    );

    await expect(fetchSonarrSeries(config, fetchMock)).resolves.toEqual({
      status: "ok",
      value: [
        {
          guid: "sonarr:12",
          title: "Severance",
          year: 2022,
          mediaType: "tv",
          externalIds: { tvdb: 371980, tmdb: 95396, imdb: "tt11280740" },
        },
      ],
    });
  });

  it("looks up by TMDB, adds with search enabled, and verifies by TVDB", async () => {
    const lookup = { title: "Severance", year: 2022, tvdbId: 371980 };
    const fetchMock = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(json([]))
      .mockResolvedValueOnce(json([lookup]))
      .mockResolvedValueOnce(json({ id: 12, ...lookup }, 201))
      .mockResolvedValueOnce(json([{ id: 12, titleSlug: "severance", ...lookup }]));

    await expect(addSonarrSeries(config, 95396, fetchMock)).resolves.toEqual({
      result: "added",
      titleSlug: "severance",
    });
    expect(fetchMock.mock.calls[1][0].toString()).toBe(
      "http://sonarr:8989/api/v3/series/lookup?term=tmdb%3A95396",
    );
    const [url, init] = fetchMock.mock.calls[2];
    expect(url.toString()).toBe("http://sonarr:8989/api/v3/series");
    expect(init?.method).toBe("POST");
    expect(JSON.parse(String(init?.body))).toMatchObject({
      tvdbId: 371980,
      qualityProfileId: 7,
      rootFolderPath: "/tv",
      monitored: true,
      seasonFolder: true,
      addOptions: { searchForMissingEpisodes: true },
    });
  });

  it("reports already tracked by TMDB without a lookup", async () => {
    const existing = vi.fn<typeof fetch>().mockResolvedValue(
      json([
        {
          id: 12,
          title: "Severance",
          titleSlug: "severance",
          tvdbId: 371980,
          tmdbId: 95396,
        },
      ]),
    );
    await expect(addSonarrSeries(config, 95396, existing)).resolves.toEqual({
      result: "already_exists",
      titleSlug: "severance",
    });
    expect(existing).toHaveBeenCalledTimes(1);
  });

  it("recognizes an existing series by the TVDB id returned from lookup", async () => {
    const existing = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(json([{ id: 12, title: "Severance", tvdbId: 371980 }]))
      .mockResolvedValueOnce(json([{ title: "Severance", tvdbId: 371980 }]));

    await expect(addSonarrSeries(config, 95396, existing)).resolves.toEqual({
      result: "already_exists",
      titleSlug: undefined,
    });
    expect(existing).toHaveBeenCalledTimes(2);
  });

  it("reports a lookup miss distinctly", async () => {
    const missing = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(json([]))
      .mockResolvedValueOnce(json([]));
    await expect(addSonarrSeries(config, 1, missing)).resolves.toEqual({
      result: "not_found",
    });
  });

  it("does not claim success until the write is visible", async () => {
    const lookup = { title: "Severance", tvdbId: 371980 };
    const fetchMock = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(json([]))
      .mockResolvedValueOnce(json([lookup]))
      .mockResolvedValueOnce(json({ id: 12, ...lookup }, 201))
      .mockResolvedValueOnce(json([]));

    await expect(addSonarrSeries(config, 95396, fetchMock)).resolves.toEqual({
      result: "error",
    });
  });
});
