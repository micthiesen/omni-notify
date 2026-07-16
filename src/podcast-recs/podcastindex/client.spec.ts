import { describe, expect, it } from "vitest";
import { mapEpisode, type RawPodcastIndexEpisode } from "./client.js";

function rawEpisode(
  overrides: Partial<RawPodcastIndexEpisode> = {},
): RawPodcastIndexEpisode {
  return {
    title: "Episode Title",
    feedTitle: "Show Title",
    feedUrl: "https://example.com/feed.xml",
    feedItunesId: 12345,
    guid: "episode-guid",
    enclosureUrl: "https://example.com/episode.mp3",
    link: "https://example.com/episode",
    datePublished: 1_700_000_000,
    duration: 1_800,
    description: "Episode description",
    image: "https://example.com/episode.png",
    feedImage: "https://example.com/feed.png",
    ...overrides,
  };
}

describe("mapEpisode", () => {
  it("maps a realistic raw episode", () => {
    expect(mapEpisode(rawEpisode())).toEqual({
      title: "Episode Title",
      feedTitle: "Show Title",
      feedUrl: "https://example.com/feed.xml",
      feedItunesId: 12345,
      guid: "episode-guid",
      enclosureUrl: "https://example.com/episode.mp3",
      episodeUrl: "https://example.com/episode",
      publishedAt: 1_700_000_000_000,
      durationMinutes: 30,
      description: "Episode description",
      artworkUrl: "https://example.com/episode.png",
    });
  });

  it("multiplies datePublished (seconds) by 1000 for publishedAt (ms)", () => {
    const episode = mapEpisode(rawEpisode({ datePublished: 1_600_000_000 }));
    expect(episode?.publishedAt).toBe(1_600_000_000_000);
  });

  it("rounds durationMinutes from duration in seconds", () => {
    const episode = mapEpisode(rawEpisode({ duration: 125 }));
    expect(episode?.durationMinutes).toBe(2);
  });

  it("omits durationMinutes when duration is absent or zero", () => {
    expect(
      mapEpisode(rawEpisode({ duration: undefined }))?.durationMinutes,
    ).toBeUndefined();
    expect(mapEpisode(rawEpisode({ duration: 0 }))?.durationMinutes).toBeUndefined();
  });

  it("falls back artworkUrl from image to feedImage", () => {
    const episode = mapEpisode(rawEpisode({ image: undefined }));
    expect(episode?.artworkUrl).toBe("https://example.com/feed.png");
  });

  it("omits artworkUrl when neither image nor feedImage is set", () => {
    const episode = mapEpisode(rawEpisode({ image: undefined, feedImage: undefined }));
    expect(episode?.artworkUrl).toBeUndefined();
  });

  it("omits feedItunesId when 0", () => {
    const episode = mapEpisode(rawEpisode({ feedItunesId: 0 }));
    expect(episode?.feedItunesId).toBeUndefined();
  });

  it("omits feedItunesId when absent", () => {
    const episode = mapEpisode(rawEpisode({ feedItunesId: undefined }));
    expect(episode?.feedItunesId).toBeUndefined();
  });

  it("sets episodeUrl from link", () => {
    const episode = mapEpisode(rawEpisode({ link: "https://example.com/ep/1" }));
    expect(episode?.episodeUrl).toBe("https://example.com/ep/1");
  });

  // Missing a required field (feedUrl, enclosureUrl, or datePublished) returns
  // undefined — the skip signal callers use to filter unusable items out of
  // the mapped list rather than throwing.
  it("returns undefined when enclosureUrl is missing", () => {
    expect(mapEpisode(rawEpisode({ enclosureUrl: undefined }))).toBeUndefined();
  });

  it("returns undefined when feedUrl is missing", () => {
    expect(mapEpisode(rawEpisode({ feedUrl: undefined }))).toBeUndefined();
  });

  it("returns undefined when datePublished is missing", () => {
    expect(mapEpisode(rawEpisode({ datePublished: undefined }))).toBeUndefined();
  });

  it("returns undefined when guid is missing (episode identity)", () => {
    expect(mapEpisode(rawEpisode({ guid: undefined }))).toBeUndefined();
    expect(mapEpisode(rawEpisode({ guid: null }))).toBeUndefined();
  });
});
