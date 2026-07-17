import { describe, expect, it } from "vitest";
import {
  filterEligibleEpisodes,
  type PodcastFilterContext,
  RECENT_EPISODE_WINDOW_MS,
} from "./filters.js";
import { normalizeTitle } from "./titles.js";
import type { EpisodeCandidate } from "./types.js";

const NOW = Date.UTC(2026, 6, 16);
const DAY = 24 * 60 * 60 * 1000;

function candidate(overrides: Partial<EpisodeCandidate> = {}): EpisodeCandidate {
  return {
    episodeId: "itunes:1#guid-1",
    showId: "itunes:1",
    showTitle: "The Gray Area",
    episodeTitle: "What is consciousness?",
    feedUrl: "https://feeds.example.com/grayarea",
    itunesId: 1,
    episodeGuid: "guid-1",
    publishedAt: NOW - 2 * DAY,
    description: "A conversation about minds.",
    showGenres: ["Philosophy"],
    discoveredVia: "reddit thread",
    ...overrides,
  };
}

function context(overrides: Partial<PodcastFilterContext> = {}): PodcastFilterContext {
  return {
    now: NOW,
    subscribedShowIds: new Set(),
    subscribedShowTitles: new Set(),
    exclusions: { episodeIds: new Set(), showIds: new Set() },
    ...overrides,
  };
}

describe("filterEligibleEpisodes", () => {
  it("keeps a fresh, unexcluded episode", () => {
    const { kept, dropped } = filterEligibleEpisodes([candidate()], context());
    expect(kept).toHaveLength(1);
    expect(dropped).toHaveLength(0);
  });

  it("drops episodes outside the recency window", () => {
    const stale = candidate({
      publishedAt: NOW - RECENT_EPISODE_WINDOW_MS - DAY,
    });
    const { kept, dropped } = filterEligibleEpisodes([stale], context());
    expect(kept).toHaveLength(0);
    expect(dropped[0].reason).toContain("outside recency window");
  });

  it("drops episodes with a future release date", () => {
    const future = candidate({ publishedAt: NOW + 2 * DAY });
    const { dropped } = filterEligibleEpisodes([future], context());
    expect(dropped[0].reason).toContain("future");
  });

  it("drops already-recommended episodes", () => {
    const { dropped } = filterEligibleEpisodes(
      [candidate()],
      context({
        exclusions: { episodeIds: new Set(["itunes:1#guid-1"]), showIds: new Set() },
      }),
    );
    expect(dropped[0].reason).toBe("episode already recommended");
  });

  it("drops shows on cooldown or excluded by feedback", () => {
    const { dropped } = filterEligibleEpisodes(
      [candidate()],
      context({
        exclusions: { episodeIds: new Set(), showIds: new Set(["itunes:1"]) },
      }),
    );
    expect(dropped[0].reason).toBe("show on cooldown or excluded by feedback");
  });

  it("drops subscribed shows by canonical id", () => {
    const { dropped } = filterEligibleEpisodes(
      [candidate()],
      context({ subscribedShowIds: new Set(["itunes:1"]) }),
    );
    expect(dropped[0].reason).toBe("already subscribed");
  });

  it("drops subscribed shows by normalized title when ids are missing", () => {
    const { dropped } = filterEligibleEpisodes(
      [candidate()],
      context({ subscribedShowTitles: new Set([normalizeTitle("The Gray Area")]) }),
    );
    expect(dropped[0].reason).toBe("already subscribed (title match)");
  });
});

describe("normalizeTitle", () => {
  it("ignores case, punctuation, and diacritics", () => {
    expect(normalizeTitle("Séan Carroll's Mindscape!")).toBe(
      normalizeTitle("sean carrolls mindscape"),
    );
  });

  it("collapses whitespace", () => {
    expect(normalizeTitle("The  Rest   Is History")).toBe("the rest is history");
  });
});
