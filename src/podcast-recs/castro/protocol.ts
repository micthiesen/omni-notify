import { z } from "zod";

export enum CastroActionType {
  ClearEpisodeNew = "clear_episode_new",
  EpisodeDequeued = "episode_dequeued",
  EpisodeLastPlayed = "episode_last_played",
  EpisodeNew = "episode_new",
  EpisodeProgress = "episode_progress",
  EpisodeQueued = "episode_queued",
}

export enum CastroActionSource {
  Policy = "policy",
  User = "user",
}

export const castroActionSchema = z.object({
  id: z.number().int().nonnegative(),
  episode_id: z.string().uuid(),
  origin_event_id: z.string().uuid(),
  origin_timestamp: z.number().int().positive(),
  source: z.enum(CastroActionSource),
  action_type: z.enum(CastroActionType),
  /** Action-specific JSON encoded as a string inside the outer JSON body. */
  event_data: z.string().optional(),
});

export type CastroAction = z.infer<typeof castroActionSchema>;

export const castroActionBatchSchema = z.object({
  actions: z.array(castroActionSchema).min(1),
});
export type CastroActionBatch = z.infer<typeof castroActionBatchSchema>;

export const castroSyncStatusSchema = z.object({
  device_status: z.number().int(),
  account_status: z.number().int(),
  latest_event_id: z.number().int().nonnegative(),
});
export type CastroSyncStatus = z.infer<typeof castroSyncStatusSchema>;

export const castroEventsResponseSchema = z.object({
  events: z.array(z.unknown()),
  latest_event_id: z.number().int().nonnegative(),
});
export type CastroEventsResponse = z.infer<typeof castroEventsResponseSchema>;

export const castroUserEventsResponseSchema = z.object({
  user_events: z.array(z.unknown()),
  latest_event_id: z.number().int().nonnegative(),
});
export type CastroUserEventsResponse = z.infer<typeof castroUserEventsResponseSchema>;

export const castroSubscriptionRequestSchema = z.object({
  feed_ids: z.array(z.string().uuid()).min(1),
});

export const castroSubscribedFeedSchema = z.object({
  feed_id: z.string().uuid(),
  feed_url: z.string().url(),
});

export const castroSubscriptionResponseSchema = z.object({
  subscribed: z.array(castroSubscribedFeedSchema),
  latest_event_id: z.number().int().nonnegative(),
});
export type CastroSubscriptionResponse = z.infer<
  typeof castroSubscriptionResponseSchema
>;

export const castroPodcastStateSchema = z.object({
  public_id: z.string().uuid(),
  episode_states: z.array(
    z.object({
      episode_id: z.string().uuid(),
      is_new: z.boolean(),
      is_starred: z.boolean(),
      is_played: z.boolean(),
      last_played: z.string().nullable(),
      progress_seconds: z.number().nonnegative(),
    }),
  ),
});
export type CastroPodcastState = z.infer<typeof castroPodcastStateSchema>;

export const castroProfileSubscriptionSchema = z.object({
  podcast_id: z.string().uuid(),
  private: z.boolean(),
  will_notify_device: z.boolean(),
});
export const castroProfileSubscriptionsSchema = z.array(
  castroProfileSubscriptionSchema,
);
export type CastroProfileSubscription = z.infer<typeof castroProfileSubscriptionSchema>;

export const castroQueueItemSchema = z.object({
  fractional_position: z.string().min(1),
  episode_id: z.string().uuid(),
  podcast_id: z.string().uuid(),
});
export const castroQueueSchema = z.object({
  queue_items: z.array(castroQueueItemSchema),
});
export type CastroQueue = z.infer<typeof castroQueueSchema>;

export const castroPodcastSearchResultSchema = z.object({
  artwork_url: z.object({
    large: z.string().url(),
    medium: z.string().url(),
    small: z.string().url(),
  }),
  author: z.string().nullable(),
  explicit: z.string(),
  feed_url: z.string().url(),
  itunes_id: z.number().int(),
  last_episode_date: z.string().nullable(),
  result_position: z.number().int().nonnegative(),
  summary: z.string().nullable(),
  tentacles_id: z.string().uuid(),
  title: z.string(),
});
export const castroPodcastSearchResultsSchema = z.array(
  castroPodcastSearchResultSchema,
);
export type CastroPodcastSearchResult = z.infer<typeof castroPodcastSearchResultSchema>;

export const castroEpisodeSearchResultSchema = z.object({
  artwork_url: z.string().url().nullable(),
  author: z.string().nullable(),
  podcast_artwork_url: z.string().url().nullable(),
  podcast_name: z.string(),
  published_at: z.string(),
  tentacles_id: z.string().uuid(),
  title: z.string(),
});
export const castroEpisodeSearchResultsSchema = z.array(
  castroEpisodeSearchResultSchema,
);
export type CastroEpisodeSearchResult = z.infer<typeof castroEpisodeSearchResultSchema>;

export const castroEpisodeSchema = z.object({
  guid: z.string(),
  public_id: z.string().uuid(),
  short_id: z.string(),
  title: z.string(),
  media_size: z
    .union([z.number(), z.object({ bytes: z.number().nonnegative() })])
    .nullable(),
  media_url: z.string().url(),
  artwork_url: z.string().url().nullable(),
  author_name: z.string().nullable(),
  link_url: z.string().url().nullable(),
  // Castro uses a negative sentinel when duration has not been discovered yet.
  duration: z.object({ seconds: z.number() }),
  description: z.string(),
  published_at: z.string(),
  predecessor_public_id: z.string().uuid().nullable(),
  season_number: z.number().int().nullable(),
  episode_number: z.number().int().nullable(),
  episode_type: z.string(),
  people: z.array(z.unknown()),
});
export type CastroEpisode = z.infer<typeof castroEpisodeSchema>;

export const castroPodcastSchema = z.object({
  public_id: z.string().uuid(),
  short_id: z.string(),
  title: z.string(),
  sort_title: z.string(),
  site_url: z.string().url().nullable(),
  description: z.string(),
  author_name: z.string().nullable(),
  artwork_url: z.string().url().nullable(),
  last_event_number: z.number().int().nonnegative(),
  podcast_type: z.string(),
  itunes_category: z.string().nullable(),
  itunes_subcategory: z.string().nullable(),
  private: z.boolean(),
  funding_text: z.string(),
  funding_url: z.string().url().nullable(),
  episodes: z.array(castroEpisodeSchema),
  people: z.array(z.unknown()),
});
export type CastroPodcast = z.infer<typeof castroPodcastSchema>;

export const castroQueueEventDataSchema = z.object({
  fractional_position: z.string().min(1),
});

export const castroLastPlayedEventDataSchema = z.object({
  last_played: z.number().int().positive(),
});

export const castroProgressEventDataSchema = z.object({
  seconds: z.number().nonnegative(),
});

export function parseCastroEventData<T>(action: CastroAction, schema: z.ZodType<T>): T {
  if (action.event_data === undefined) {
    throw new Error(`${action.action_type} has no event_data`);
  }
  return schema.parse(JSON.parse(action.event_data));
}
