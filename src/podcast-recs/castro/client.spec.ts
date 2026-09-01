import type { Logger } from "@micthiesen/mitools/logging";
import { Effect } from "effect";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { EnqueueEpisodeRequest } from "../account.js";
import { PodcastQueuePosition } from "../account.js";
import type { CastroApi } from "./api.js";
import {
  CastroClient,
  matchEpisode,
  normalizeMediaUrl,
  sharedCastroApi,
} from "./client.js";
import {
  type CastroAction,
  CastroActionType,
  type CastroEpisode,
  type CastroPodcast,
} from "./protocol.js";

const PODCAST_ID = "33333333-3333-4333-8333-333333333333";
const EPISODE_ID = "11111111-1111-4111-8111-111111111111";
const FEED_URL = "https://example.com/feed.xml";

describe("sharedCastroApi", () => {
  it("reuses pacing for unchanged credentials", () => {
    expect(sharedCastroApi("device-a", "secret-a")).toBe(
      sharedCastroApi("device-a", "secret-a"),
    );
  });

  it("does not retain an API signed with stale credentials", () => {
    const original = sharedCastroApi("device-a", "secret-a");
    expect(sharedCastroApi("device-b", "secret-b")).not.toBe(original);
  });
});

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
    searchPodcasts: vi.fn(() => Effect.succeed([searchResult])),
    searchEpisodes: vi.fn(() =>
      Effect.succeed([
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
    ),
    fetchPodcast: vi.fn(() => Effect.succeed(podcast)),
    fetchSubscriptions: vi.fn(() => Effect.succeed([])),
    fetchQueue: vi.fn(
      (): Effect.Effect<{
        queue_items: {
          episode_id: string;
          podcast_id: string;
          fractional_position: string;
        }[];
      }> => Effect.succeed({ queue_items: [] }),
    ),
    postActions: vi.fn((_actions: CastroAction[]) => Effect.void),
    subscribe: vi.fn(() =>
      Effect.succeed({
        subscribed: [{ feed_id: PODCAST_ID, feed_url: FEED_URL }],
        latest_event_id: 1,
      }),
    ),
  };
}

const logger = { error: vi.fn() } as unknown as Logger;

describe("CastroClient search-backed writes", () => {
  beforeEach(() => vi.clearAllMocks());

  it("maps general podcast and episode searches", async () => {
    const api = fakeApi();
    const client = await Effect.runPromise(
      CastroClient.make(api as unknown as CastroApi, logger),
    );

    await expect(
      Effect.runPromise(client.searchPodcasts("example")),
    ).resolves.toMatchObject({
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
    await expect(
      Effect.runPromise(client.searchEpisodes("example episode")),
    ).resolves.toMatchObject({
      status: "ok",
      value: [{ clientId: EPISODE_ID, showTitle: "Example Podcast" }],
    });
  });

  it("resolves an unsubscribed RSS feed and enqueues its episode", async () => {
    const api = fakeApi();
    const client = await Effect.runPromise(
      CastroClient.make(api as unknown as CastroApi, logger),
    );

    await expect(
      Effect.runPromise(
        client.enqueueEpisode({
          feedUrl: FEED_URL,
          itunesId: 1234,
          episodeGuid: "rss-guid",
          showTitle: "Example Podcast",
          episodeTitle: "Example Episode",
          position: PodcastQueuePosition.Last,
        }),
      ),
    ).resolves.toBe("added");

    expect(api.searchPodcasts).toHaveBeenCalledWith(FEED_URL);
    expect(api.fetchPodcast).toHaveBeenCalledWith(PODCAST_ID);
    expect(api.postActions).toHaveBeenCalledOnce();
    expect(
      api.postActions.mock.calls[0]?.[0].map((action) => action.action_type),
    ).toEqual([CastroActionType.EpisodeQueued, CastroActionType.ClearEpisodeNew]);
  });

  it("allocates unique action ids across overlapping client instances", async () => {
    const firstApi = fakeApi();
    const secondApi = fakeApi();
    const [first, second] = await Effect.runPromise(
      Effect.all([
        CastroClient.make(firstApi as unknown as CastroApi, logger),
        CastroClient.make(secondApi as unknown as CastroApi, logger),
      ]),
    );
    const request: EnqueueEpisodeRequest = {
      feedUrl: FEED_URL,
      episodeGuid: "rss-guid",
      showTitle: "Example Podcast",
      episodeTitle: "Example Episode",
    };

    await Effect.runPromise(
      Effect.all([first.enqueueEpisode(request), second.enqueueEpisode(request)], {
        concurrency: "unbounded",
      }),
    );

    const ids = [
      ...(firstApi.postActions.mock.calls[0]?.[0] ?? []),
      ...(secondApi.postActions.mock.calls[0]?.[0] ?? []),
    ].map(({ id }) => id);
    expect(new Set(ids).size).toBe(4);
  });

  it("serializes concurrent queue read/decision/write across client instances", async () => {
    let queued = false;
    const api = fakeApi();
    api.fetchQueue = vi.fn(() =>
      Effect.succeed({
        queue_items: queued
          ? [
              {
                episode_id: EPISODE_ID,
                podcast_id: PODCAST_ID,
                fractional_position: "a0",
              },
            ]
          : [],
      }),
    );
    api.postActions = vi.fn(() =>
      Effect.sync(() => {
        queued = true;
      }),
    );
    const [first, second] = await Effect.runPromise(
      Effect.all([
        CastroClient.make(api as unknown as CastroApi, logger),
        CastroClient.make(api as unknown as CastroApi, logger),
      ]),
    );
    const request: EnqueueEpisodeRequest = {
      feedUrl: FEED_URL,
      episodeGuid: "rss-guid",
      showTitle: "Example Podcast",
      episodeTitle: "Example Episode",
    };

    const results = await Effect.runPromise(
      Effect.all([first.enqueueEpisode(request), second.enqueueEpisode(request)], {
        concurrency: "unbounded",
      }),
    );

    expect(results.sort()).toEqual(["added", "already_exists"]);
    expect(api.postActions).toHaveBeenCalledTimes(1);
    expect(api.fetchQueue).toHaveBeenCalledTimes(2);
  });

  it("Queue Next inserts after the current top item, not above it", async () => {
    const api = fakeApi();
    // A queue whose top item ("a0") is playing/up-next and a second item ("a1").
    api.fetchQueue = vi.fn(() =>
      Effect.succeed({
        queue_items: [
          { episode_id: "e0", podcast_id: PODCAST_ID, fractional_position: "a0" },
          { episode_id: "e1", podcast_id: PODCAST_ID, fractional_position: "a1" },
        ],
      }),
    );
    const client = await Effect.runPromise(
      CastroClient.make(api as unknown as CastroApi, logger),
    );

    await Effect.runPromise(
      client.enqueueEpisode({
        feedUrl: FEED_URL,
        episodeGuid: "rss-guid",
        showTitle: "Example Podcast",
        episodeTitle: "Example Episode",
        position: PodcastQueuePosition.Next,
      }),
    );

    const queued = api.postActions.mock.calls[0]?.[0][0];
    const pos = JSON.parse(queued?.event_data ?? "{}").fractional_position as string;
    // Between the current top ("a0") and the second item ("a1") → new 2nd item.
    expect(pos > "a0" && pos < "a1").toBe(true);
  });

  it("resolves an RSS feed and subscribes by Castro podcast id", async () => {
    const api = fakeApi();
    const client = await Effect.runPromise(
      CastroClient.make(api as unknown as CastroApi, logger),
    );

    await expect(
      Effect.runPromise(
        client.subscribeToShow({
          title: "Example Podcast",
          feedUrl: FEED_URL,
          itunesId: 1234,
        }),
      ),
    ).resolves.toBe("added");
    expect(api.subscribe).toHaveBeenCalledWith([PODCAST_ID]);
  });
});

describe("CastroClient Inbox", () => {
  it("returns only is_new episodes and clears them without dequeueing", async () => {
    const api = {
      ...fakeApi(),
      fetchSubscriptions: vi.fn(() =>
        Effect.succeed([
          { podcast_id: PODCAST_ID, private: false, will_notify_device: true },
        ]),
      ),
      fetchPodcastState: vi.fn(() =>
        Effect.succeed({
          public_id: PODCAST_ID,
          episode_states: [
            {
              episode_id: EPISODE_ID,
              is_new: true,
              is_starred: false,
              is_played: false,
              last_played: null,
              progress_seconds: 0,
            },
            {
              episode_id: "22222222-2222-4222-8222-222222222222",
              is_new: false,
              is_starred: false,
              is_played: false,
              last_played: null,
              progress_seconds: 0,
            },
          ],
        }),
      ),
      fetchEpisode: vi.fn(() =>
        Effect.succeed(
          castroEpisode({
            public_id: EPISODE_ID,
            guid: "rss-guid",
            title: "Preview Episode",
            description: "This is a free preview of a paid post.",
          }),
        ),
      ),
    };
    const client = await Effect.runPromise(
      CastroClient.make(api as unknown as CastroApi, logger),
    );

    await expect(Effect.runPromise(client.fetchInbox())).resolves.toMatchObject({
      status: "ok",
      value: [
        {
          clientEpisodeId: EPISODE_ID,
          episodeGuid: "rss-guid",
          episodeTitle: "Preview Episode",
        },
      ],
    });
    await expect(Effect.runPromise(client.clearInboxEpisode(EPISODE_ID))).resolves.toBe(
      "removed",
    );
    expect(api.fetchQueue).not.toHaveBeenCalled();
    expect(api.postActions).toHaveBeenLastCalledWith([
      expect.objectContaining({
        episode_id: EPISODE_ID,
        action_type: CastroActionType.ClearEpisodeNew,
      }),
    ]);
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

describe("CastroClient.fetchListenHistory", () => {
  const DAY = 24 * 60 * 60 * 1000;
  const iso = (msAgo: number) => new Date(Date.now() - msAgo).toISOString();
  const EP_PLAYED = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
  const EP_PARTIAL = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
  const EP_OLD = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";

  function historyApi(episodeStates: unknown[]) {
    const episodes: Record<string, unknown> = {
      [EP_PLAYED]: {
        public_id: EP_PLAYED,
        guid: "guid-played",
        title: "Played Ep",
        media_url: "u1",
        duration: { seconds: 1000 },
        description: "",
      },
      [EP_PARTIAL]: {
        public_id: EP_PARTIAL,
        guid: "guid-partial",
        title: "Partial Ep",
        media_url: "u2",
        duration: { seconds: 100 },
        description: "",
      },
      [EP_OLD]: {
        public_id: EP_OLD,
        guid: "guid-old",
        title: "Old Ep",
        media_url: "u3",
        duration: { seconds: 100 },
        description: "",
      },
    };
    return {
      fetchSubscriptions: vi.fn(() =>
        Effect.succeed([
          { podcast_id: PODCAST_ID, private: false, will_notify_device: true },
        ]),
      ),
      fetchPodcast: vi.fn(() =>
        Effect.succeed({
          public_id: PODCAST_ID,
          title: "Example Podcast",
          episodes: [],
        }),
      ),
      fetchPodcastState: vi.fn(() =>
        Effect.succeed({
          podcast_id: PODCAST_ID,
          episode_states: episodeStates,
        }),
      ),
      fetchEpisode: vi.fn((id: string) => Effect.succeed(episodes[id])),
    };
  }

  it("computes completion and honors the sinceMs cutoff", async () => {
    const api = historyApi([
      {
        episode_id: EP_PLAYED,
        is_new: false,
        is_starred: true,
        is_played: true,
        last_played: iso(2 * DAY),
        progress_seconds: 0,
      },
      {
        episode_id: EP_PARTIAL,
        is_new: false,
        is_starred: false,
        is_played: false,
        last_played: iso(2 * DAY),
        progress_seconds: 30,
      },
      {
        episode_id: EP_OLD,
        is_new: false,
        is_starred: false,
        is_played: false,
        last_played: iso(100 * DAY),
        progress_seconds: 10,
      },
    ]);
    const client = await Effect.runPromise(
      CastroClient.make(api as unknown as CastroApi, logger),
    );

    const result = await Effect.runPromise(
      client.fetchListenHistory(Date.now() - 10 * DAY),
    );
    expect(result.status).toBe("ok");
    if (result.status !== "ok") return;
    // The 100-day-old play is excluded by the cutoff.
    expect(result.value).toHaveLength(2);
    const played = result.value.find((e) => e.episodeGuid === "guid-played");
    const partial = result.value.find((e) => e.episodeGuid === "guid-partial");
    expect(played?.completion).toBe(1);
    expect(played?.starred).toBe(true);
    expect(played?.mediaUrl).toBe("u1");
    expect(partial?.completion).toBeCloseTo(0.3);
  });

  it("clamps completion to 1 when progress exceeds duration", async () => {
    const api = historyApi([
      {
        episode_id: EP_PARTIAL,
        is_new: false,
        is_starred: false,
        is_played: false,
        last_played: iso(DAY),
        progress_seconds: 500,
      },
    ]);
    const client = await Effect.runPromise(
      CastroClient.make(api as unknown as CastroApi, logger),
    );

    const result = await Effect.runPromise(client.fetchListenHistory());
    if (result.status !== "ok") throw new Error("expected ok");
    expect(result.value[0]?.completion).toBe(1);
  });

  it("skips episodes that were never played (null last_played)", async () => {
    const api = historyApi([
      {
        episode_id: EP_PLAYED,
        is_new: true,
        is_starred: false,
        is_played: false,
        last_played: null,
        progress_seconds: 0,
      },
    ]);
    const client = await Effect.runPromise(
      CastroClient.make(api as unknown as CastroApi, logger),
    );

    const result = await Effect.runPromise(client.fetchListenHistory());
    if (result.status !== "ok") throw new Error("expected ok");
    expect(result.value).toHaveLength(0);
  });
});

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
