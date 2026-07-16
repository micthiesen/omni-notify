import type { Logger } from "@micthiesen/mitools/logging";
import { describe, expect, it, vi } from "vitest";
import type { PodcastAccountClient } from "./account.js";
import { pickBestByTitle, resolveCandidates } from "./candidates.js";
import { pickBestShowMatch, searchItunesPodcasts } from "./itunes.js";
import { fetchFeedEpisodes, findEpisodeByTitle } from "./rss.js";
import type { DiscoveredEpisode } from "./types.js";

vi.mock("./itunes.js");
vi.mock("./rss.js");

const logger = { info: vi.fn(), warn: vi.fn() } as unknown as Logger;

function discovered(overrides: Partial<DiscoveredEpisode> = {}): DiscoveredEpisode {
  return { showTitle: "Show", episodeTitle: "Ep", context: "reddit", ...overrides };
}

function feedEpisode() {
  return {
    guid: "rss-guid",
    title: "Ep",
    publishedAt: 1_700_000_000_000,
    description: "desc",
    enclosureUrl: "https://cdn/x.mp3",
    link: "https://show/ep",
  };
}

describe("resolveCandidates", () => {
  it("falls back to Castro search when iTunes cannot place the show", async () => {
    vi.mocked(searchItunesPodcasts).mockResolvedValue([]);
    vi.mocked(pickBestShowMatch).mockReturnValue(undefined);
    vi.mocked(fetchFeedEpisodes).mockResolvedValue([feedEpisode()]);
    vi.mocked(findEpisodeByTitle).mockReturnValue(feedEpisode());

    const account = {
      searchPodcasts: vi.fn(async () => ({
        status: "ok" as const,
        value: [
          {
            clientId: "c1",
            title: "Show",
            feedUrl: "https://feeds/show",
            itunesId: 42,
            artworkUrl: "https://art",
          },
        ],
      })),
    } as unknown as PodcastAccountClient;

    const result = await resolveCandidates([discovered()], account, logger);
    expect(result).toHaveLength(1);
    expect(result[0]).toMatchObject({
      showId: "itunes:42",
      feedUrl: "https://feeds/show",
      itunesId: 42,
      mediaUrl: "https://cdn/x.mp3",
      showGenres: [],
    });
    expect(account.searchPodcasts).toHaveBeenCalledWith("Show");
  });

  it("drops the candidate when iTunes misses and no account is available", async () => {
    vi.mocked(searchItunesPodcasts).mockResolvedValue([]);
    vi.mocked(pickBestShowMatch).mockReturnValue(undefined);

    const result = await resolveCandidates([discovered()], undefined, logger);
    expect(result).toHaveLength(0);
  });

  it("isolates per-item failures and dedupes by episodeId", async () => {
    vi.mocked(searchItunesPodcasts).mockResolvedValue([]);
    vi.mocked(pickBestShowMatch).mockReturnValue({
      itunesId: 7,
      title: "Show",
      feedUrl: "https://feeds/show",
      genres: ["News"],
    });
    // First discovered item resolves; second throws; third duplicates the first.
    vi.mocked(fetchFeedEpisodes)
      .mockResolvedValueOnce([feedEpisode()])
      .mockRejectedValueOnce(new Error("feed down"))
      .mockResolvedValueOnce([feedEpisode()]);
    vi.mocked(findEpisodeByTitle).mockReturnValue(feedEpisode());

    const result = await resolveCandidates(
      [discovered(), discovered({ episodeTitle: "Boom" }), discovered()],
      undefined,
      logger,
    );
    expect(result).toHaveLength(1);
    expect(result[0]?.showId).toBe("itunes:7");
  });
});

describe("pickBestByTitle", () => {
  const shows = [
    { title: "The Gray Area with Sean Illing", feedUrl: "a" },
    { title: "Past Present Future", feedUrl: "b" },
    { title: "Very Bad Wizards", feedUrl: "c" },
  ];

  it("prefers an exact normalized match", () => {
    expect(pickBestByTitle(shows, "very bad wizards!")?.feedUrl).toBe("c");
  });

  it("falls back to containment in either direction", () => {
    expect(pickBestByTitle(shows, "The Gray Area")?.feedUrl).toBe("a");
    expect(pickBestByTitle(shows, "Past, Present & Future")?.feedUrl).toBe("b");
  });

  it("returns undefined when nothing matches", () => {
    expect(pickBestByTitle(shows, "Hardcore History")).toBeUndefined();
  });
});
