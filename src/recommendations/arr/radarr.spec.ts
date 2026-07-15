import { describe, expect, it, vi } from "vitest";
import type { ArrConfig } from "./client.js";
import { addRadarrMovie, fetchRadarrMovies } from "./radarr.js";

const config: ArrConfig = {
  url: "http://radarr:7878",
  apiKey: "radarr-key",
  rootFolderPath: "/movies",
  qualityProfileId: 4,
};

function json(value: unknown, status = 200): Response {
  return new Response(JSON.stringify(value), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

describe("Radarr adapter", () => {
  it("normalizes tracked movies", async () => {
    const fetchMock = vi.fn<typeof fetch>().mockResolvedValue(
      json([
        { id: 8, title: "Arrival", year: 2016, tmdbId: 329865, imdbId: "tt2543164" },
        { id: 9, title: "Malformed entry" },
      ]),
    );

    await expect(fetchRadarrMovies(config, fetchMock)).resolves.toEqual({
      status: "ok",
      value: [
        {
          guid: "radarr:8",
          title: "Arrival",
          year: 2016,
          mediaType: "movie",
          externalIds: { tmdb: 329865, imdb: "tt2543164" },
        },
      ],
    });
    expect(fetchMock).toHaveBeenCalledWith(
      new URL("http://radarr:7878/api/v3/movie"),
      expect.objectContaining({
        headers: expect.objectContaining({ "X-Api-Key": "radarr-key" }),
      }),
    );
  });

  it("reports an existing movie without looking it up or writing", async () => {
    const fetchMock = vi
      .fn<typeof fetch>()
      .mockResolvedValue(json([{ id: 8, title: "Arrival", tmdbId: 329865 }]));

    await expect(addRadarrMovie(config, 329865, fetchMock)).resolves.toBe(
      "already_exists",
    );
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it("adds a looked-up movie with acquisition defaults and verifies it", async () => {
    const lookup = { title: "Arrival", year: 2016, tmdbId: 329865 };
    const fetchMock = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(json([]))
      .mockResolvedValueOnce(json(lookup))
      .mockResolvedValueOnce(json({ id: 8, ...lookup }, 201))
      .mockResolvedValueOnce(json([{ id: 8, ...lookup }]));

    await expect(addRadarrMovie(config, 329865, fetchMock)).resolves.toBe("added");
    const [url, init] = fetchMock.mock.calls[2];
    expect(url.toString()).toBe("http://radarr:7878/api/v3/movie");
    expect(init?.method).toBe("POST");
    expect(JSON.parse(String(init?.body))).toMatchObject({
      tmdbId: 329865,
      qualityProfileId: 4,
      rootFolderPath: "/movies",
      monitored: true,
      addOptions: { searchForMovie: true },
    });
  });

  it("distinguishes lookup misses, service failure, and rejected writes", async () => {
    const missing = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(json([]))
      .mockResolvedValueOnce(json({}));
    await expect(addRadarrMovie(config, 1, missing)).resolves.toBe("not_found");

    const unavailable = vi.fn<typeof fetch>().mockRejectedValue(new Error("offline"));
    await expect(addRadarrMovie(config, 1, unavailable)).resolves.toBe("unavailable");

    const rejected = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(json([]))
      .mockResolvedValueOnce(json({ title: "Movie", tmdbId: 1 }))
      .mockResolvedValueOnce(json({ message: "invalid" }, 400));
    await expect(addRadarrMovie(config, 1, rejected)).resolves.toBe("error");
  });
});
