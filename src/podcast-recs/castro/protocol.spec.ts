import { describe, expect, it } from "vitest";
import {
  CastroActionSource,
  CastroActionType,
  castroActionBatchSchema,
  castroEpisodeSearchResultsSchema,
  castroLastPlayedEventDataSchema,
  castroPodcastSearchResultsSchema,
  castroPodcastStateSchema,
  castroProfileSubscriptionsSchema,
  castroProgressEventDataSchema,
  castroQueueEventDataSchema,
  castroQueueSchema,
  castroSubscriptionResponseSchema,
  parseCastroEventData,
} from "./protocol.js";

const EPISODE_ID = "11111111-1111-4111-8111-111111111111";

function action(
  id: number,
  actionType: CastroActionType,
  eventData?: Record<string, unknown>,
) {
  return {
    id,
    episode_id: EPISODE_ID,
    origin_event_id: `22222222-2222-4222-8222-${id.toString().padStart(12, "0")}`,
    origin_timestamp: 1_784_223_113_517,
    source: CastroActionSource.User,
    action_type: actionType,
    ...(eventData ? { event_data: JSON.stringify(eventData) } : {}),
  };
}

describe("Castro sync protocol", () => {
  it("parses the captured queue-next action pair", () => {
    const batch = castroActionBatchSchema.parse({
      actions: [
        action(1, CastroActionType.EpisodeQueued, { fractional_position: "ZME" }),
        action(2, CastroActionType.ClearEpisodeNew),
      ],
    });

    expect(batch.actions.map((item) => item.action_type)).toEqual([
      CastroActionType.EpisodeQueued,
      CastroActionType.ClearEpisodeNew,
    ]);
    expect(parseCastroEventData(batch.actions[0], castroQueueEventDataSchema)).toEqual({
      fractional_position: "ZME",
    });
  });

  it("parses the captured queue-last position", () => {
    const queued = castroActionBatchSchema.parse({
      actions: [
        action(1, CastroActionType.EpisodeQueued, { fractional_position: "aE" }),
      ],
    }).actions[0];

    expect(parseCastroEventData(queued, castroQueueEventDataSchema)).toEqual({
      fractional_position: "aE",
    });
  });

  it("parses playback activity event data", () => {
    const batch = castroActionBatchSchema.parse({
      actions: [
        action(1, CastroActionType.EpisodeLastPlayed, {
          last_played: 1_784_223_188,
        }),
        action(2, CastroActionType.EpisodeProgress, {
          seconds: 3.8196825396825398,
        }),
      ],
    });

    expect(
      parseCastroEventData(batch.actions[0], castroLastPlayedEventDataSchema),
    ).toEqual({ last_played: 1_784_223_188 });
    expect(
      parseCastroEventData(batch.actions[1], castroProgressEventDataSchema),
    ).toEqual({ seconds: 3.8196825396825398 });
  });

  it("parses a captured subscription mutation response shape", () => {
    expect(
      castroSubscriptionResponseSchema.parse({
        subscribed: [
          {
            feed_id: "33333333-3333-4333-8333-333333333333",
            feed_url: "https://example.com/feed.xml",
          },
        ],
        latest_event_id: 42,
      }),
    ).toEqual({
      subscribed: [
        {
          feed_id: "33333333-3333-4333-8333-333333333333",
          feed_url: "https://example.com/feed.xml",
        },
      ],
      latest_event_id: 42,
    });
  });

  it("parses live subscription, queue, and playback state snapshots", () => {
    expect(
      castroProfileSubscriptionsSchema.parse([
        {
          podcast_id: "33333333-3333-4333-8333-333333333333",
          private: false,
          will_notify_device: true,
        },
      ]),
    ).toHaveLength(1);
    expect(
      castroQueueSchema.parse({
        queue_items: [
          {
            fractional_position: "ZM8",
            episode_id: EPISODE_ID,
            podcast_id: "33333333-3333-4333-8333-333333333333",
          },
        ],
      }).queue_items[0]?.fractional_position,
    ).toBe("ZM8");
    expect(
      castroPodcastStateSchema.parse({
        public_id: "33333333-3333-4333-8333-333333333333",
        episode_states: [
          {
            episode_id: EPISODE_ID,
            is_new: false,
            is_starred: true,
            is_played: false,
            last_played: "2026-07-16T17:30:00.000Z",
            progress_seconds: 42.5,
          },
        ],
      }).episode_states[0],
    ).toMatchObject({ progress_seconds: 42.5, is_starred: true });
  });

  it("parses captured podcast and episode search results", () => {
    expect(
      castroPodcastSearchResultsSchema.parse([
        {
          artwork_url: {
            large: "https://example.com/large.jpg",
            medium: "https://example.com/medium.jpg",
            small: "https://example.com/small.jpg",
          },
          author: "Example Author",
          explicit: "clean",
          feed_url: "https://example.com/feed.xml",
          itunes_id: 1234,
          last_episode_date: null,
          result_position: 0,
          summary: "Example podcast",
          tentacles_id: "33333333-3333-4333-8333-333333333333",
          title: "Example Podcast",
        },
      ]),
    ).toHaveLength(1);
    expect(
      castroEpisodeSearchResultsSchema.parse([
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
    ).toHaveLength(1);
  });
});
