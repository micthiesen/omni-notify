import type { Logger } from "@micthiesen/mitools/logging";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { EnqueueEpisodeRequest } from "../account.js";
import { PodcastQueuePosition } from "../account.js";
import type { CastroApi } from "./api.js";
import { CastroClient, matchEpisode, normalizeMediaUrl } from "./client.js";
import {
  type CastroAction,
  CastroActionType,
  type CastroEpisode,
  type CastroPodcast,
} from "./protocol.js";

const PODCAST_ID = "33333333-3333-4333-8333-333333333333";
const EPISODE_ID = "11111111-1111-4111-8111-111111111111";
const FEED_URL = "https://example.com/feed.xml";

const searchResult = {
  artwork_url: {
    large: "https://example.com/large.jpg",
    medium: "https://example.com/medium.jpg",
    small: "https://example.com/small.jpg",
  },
  author: "Example Author",
  explicit: "clean",
  feed_url: FEED_URL,
  itunes_id: 1234,
  last_episode_date: "2026-07-16T17:30:00.000Z",
  result_position: 0,
  summary: "Example summary",
  tentacles_id: PODCAST_ID,
  title: "Example Podcast",
};

const podcast = {
  public_id: PODCAST_ID,
  title: "Example Podcast",
  episodes: [
    {
      public_id: EPISODE_ID,
      guid: "rss-guid",
      title: "Example Episode",
    },
  ],
} as CastroPodcast;

function fakeApi() {
  return {
    searchPodcasts: vi.fn(async () => [searchResult]),
    searchEpisodes: vi.fn(async () => [
      {
        artwork_url: "https://example.com/episode.jpg",
        author: "Example Author",
        podcast_artwork_url: "https://example.com/podcast.jpg",
        podcast_name: "Example Podcast",
        published_at: "2026-07-16T17:30:00.000Z",
        tentacles_id: EPISODE_ID,
        title: "Example Episode",
      },
    ]),
    fetchPodcast: vi.fn(async () => podcast),
    fetchSubscriptions: vi.fn(async () => []),
    fetchQueue: vi.fn(async () => ({ queue_items: [] })),
    postActions: vi.fn(async (_actions: CastroAction[]) => undefined),
    subscribe: vi.fn(async () => ({
      subscribed: [{ feed_id: PODCAST_ID, feed_url: FEED_URL }],
      latest_event_id: 1,
    })),
  };
}

const logger = { error: vi.fn() } as unknown as Logger;

describe("CastroClient search-backed writes", () => {
  beforeEach(() => vi.clearAllMocks());

  it("maps general podcast and episode searches", async () => {
    const api = fakeApi();
    const client = new CastroClient(api as unknown as CastroApi, logger);

    await expect(client.searchPodcasts("example")).resolves.toMatchObject({
      status: "ok",
      value: [
        {
          clientId: PODCAST_ID,
          feedUrl: FEED_URL,
          itunesId: 1234,
          title: "Example Podcast",
        },
      ],
    });
    await expect(client.searchEpisodes("example episode")).resolves.toMatchObject({
      status: "ok",
      value: [{ clientId: EPISODE_ID, showTitle: "Example Podcast" }],
    });
  });

  it("resolves an unsubscribed RSS feed and enqueues its episode", async () => {
    const api = fakeApi();
    const client = new CastroClient(api as unknown as CastroApi, logger);

    await expect(
      client.enqueueEpisode({
        feedUrl: FEED_URL,
        itunesId: 1234,
        episodeGuid: "rss-guid",
        showTitle: "Example Podcast",
        episodeTitle: "Example Episode",
        position: PodcastQueuePosition.Last,
      }),
    ).resolves.toBe("added");

    expect(api.searchPodcasts).toHaveBeenCalledWith(FEED_URL);
    expect(api.fetchPodcast).toHaveBeenCalledWith(PODCAST_ID);
    expect(api.postActions).toHaveBeenCalledOnce();
    expect(
      api.postActions.mock.calls[0]?.[0].map((action) => action.action_type),
    ).toEqual([CastroActionType.EpisodeQueued, CastroActionType.ClearEpisodeNew]);
  });

  it("resolves an RSS feed and subscribes by Castro podcast id", async () => {
    const api = fakeApi();
    const client = new CastroClient(api as unknown as CastroApi, logger);

    await expect(
      client.subscribeToShow({
        title: "Example Podcast",
        feedUrl: FEED_URL,
        itunesId: 1234,
      }),
    ).resolves.toBe("added");
    expect(api.subscribe).toHaveBeenCalledWith([PODCAST_ID]);
  });
});

function castroEpisode(overrides: Partial<CastroEpisode>): CastroEpisode {
  return {
    guid: "castro-guid",
    public_id: EPISODE_ID,
    title: "An Episode",
    media_url: "https://cdn.example.com/audio/ep.mp3",
    duration: { seconds: 100 },
    ...overrides,
  } as unknown as CastroEpisode;
}

function enqueueRequest(
  overrides: Partial<EnqueueEpisodeRequest>,
): EnqueueEpisodeRequest {
  return {
    feedUrl: FEED_URL,
    episodeGuid: "rss-guid",
    showTitle: "A Show",
    episodeTitle: "An Episode",
    ...overrides,
  };
}

describe("normalizeMediaUrl", () => {
  it("ignores protocol and query params", () => {
    expect(normalizeMediaUrl("HTTPS://cdn.x.com/a/ep.mp3?token=1")).toBe(
      "cdn.x.com/a/ep.mp3",
    );
    expect(normalizeMediaUrl("http://cdn.x.com/a/ep.mp3")).toBe("cdn.x.com/a/ep.mp3");
  });

  it("returns undefined for empty input", () => {
    expect(normalizeMediaUrl(undefined)).toBeUndefined();
    expect(normalizeMediaUrl("  ")).toBeUndefined();
  });
});

describe("matchEpisode", () => {
  it("matches by guid first", () => {
    const eps = [castroEpisode({ guid: "rss-guid", public_id: "p1" })];
    expect(matchEpisode(eps, enqueueRequest({}))?.public_id).toBe("p1");
  });

  it("falls back to the enclosure URL when guids differ", () => {
    const eps = [
      castroEpisode({
        guid: "castro-only",
        public_id: "p2",
        media_url: "https://prefix.fm/redirect/cdn.x.com/audio/ep.mp3?aid=rss",
      }),
    ];
    const req = enqueueRequest({
      episodeGuid: "different-rss-guid",
      mediaUrl: "https://prefix.fm/redirect/cdn.x.com/audio/ep.mp3?aid=other",
    });
    expect(matchEpisode(eps, req)?.public_id).toBe("p2");
  });

  it("falls back to a unique title match", () => {
    const eps = [
      castroEpisode({ guid: "g1", public_id: "p3", title: "The Holy Shiver" }),
    ];
    const req = enqueueRequest({
      episodeGuid: "nope",
      mediaUrl: undefined,
      episodeTitle: "The Holy Shiver!",
    });
    expect(matchEpisode(eps, req)?.public_id).toBe("p3");
  });

  it("refuses an ambiguous title match", () => {
    const eps = [
      castroEpisode({ guid: "g1", public_id: "p4", title: "Bonus", media_url: "a" }),
      castroEpisode({ guid: "g2", public_id: "p5", title: "Bonus", media_url: "b" }),
    ];
    const req = enqueueRequest({
      episodeGuid: "nope",
      mediaUrl: undefined,
      episodeTitle: "Bonus",
    });
    expect(matchEpisode(eps, req)).toBeUndefined();
  });
});
