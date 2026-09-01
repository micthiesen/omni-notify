import { describe, expect, it, vi } from "vitest";
import { Cause, Effect, Exit, Fiber } from "effect";
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

    await expect(
      Effect.runPromise(fetchSonarrSeries(config, fetchMock)),
    ).resolves.toEqual({
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

  it("treats malformed tracked-series payloads as unavailable", async () => {
    const nullPayload = vi.fn<typeof fetch>().mockResolvedValue(json(null));
    await expect(
      Effect.runPromise(fetchSonarrSeries(config, nullPayload)),
    ).resolves.toEqual({ status: "unavailable" });

    const malformedEntry = vi
      .fn<typeof fetch>()
      .mockResolvedValue(json([{ id: 12, title: "Severance", tvdbId: 371980 }, null]));
    await expect(
      Effect.runPromise(fetchSonarrSeries(config, malformedEntry)),
    ).resolves.toEqual({ status: "unavailable" });
  });

  it("cancels an in-progress response read when interrupted", async () => {
    const pull = vi.fn();
    const cancelled = vi.fn();
    const body = new ReadableStream<Uint8Array>({ pull, cancel: cancelled });
    const fetchMock = vi
      .fn<typeof fetch>()
      .mockResolvedValue(
        new Response(body, { headers: { "Content-Type": "application/json" } }),
      );
    const fiber = Effect.runFork(fetchSonarrSeries(config, fetchMock));
    await vi.waitFor(() => expect(pull).toHaveBeenCalled());

    await Effect.runPromise(Fiber.interrupt(fiber));
    const exit = await Effect.runPromise(Fiber.await(fiber));

    expect(Exit.isFailure(exit) && Cause.hasInterrupts(exit.cause)).toBe(true);
    expect(cancelled).toHaveBeenCalledTimes(1);
  });

  it("looks up by TMDB, adds with search enabled, and verifies by TVDB", async () => {
    const lookup = { title: "Severance", year: 2022, tvdbId: 371980 };
    const fetchMock = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(json([]))
      .mockResolvedValueOnce(json([lookup]))
      .mockResolvedValueOnce(json({ id: 12, ...lookup }, 201))
      .mockResolvedValueOnce(json([{ id: 12, titleSlug: "severance", ...lookup }]));

    await expect(
      Effect.runPromise(addSonarrSeries(config, 95396, fetchMock)),
    ).resolves.toEqual({
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
    await expect(
      Effect.runPromise(addSonarrSeries(config, 95396, existing)),
    ).resolves.toEqual({
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

    await expect(
      Effect.runPromise(addSonarrSeries(config, 95396, existing)),
    ).resolves.toEqual({
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
    await expect(
      Effect.runPromise(addSonarrSeries(config, 1, missing)),
    ).resolves.toEqual({
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

    await expect(
      Effect.runPromise(addSonarrSeries(config, 95396, fetchMock)),
    ).resolves.toEqual({
      result: "error",
    });
  });

  it("does not inspect properties on a malformed lookup response", async () => {
    const malformed = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(json([]))
      .mockResolvedValueOnce(json([null]));
    await expect(
      Effect.runPromise(addSonarrSeries(config, 95396, malformed)),
    ).resolves.toEqual({ result: "unavailable" });
  });
});
