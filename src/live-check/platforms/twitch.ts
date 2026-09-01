import { Effect, Schema } from "effect";
import { fetchGQL } from "./common.js";
import { type FetchedStatus, LiveStatus } from "./index.js";

const TWITCH_GQL_URL = "https://gql.twitch.tv/gql";

// Public client ID used by Twitch's web player and many open source projects.
// No authentication required. See: https://github.com/nicknsy/twitch-api/wiki/Public-GraphQL-queries
const TWITCH_CLIENT_ID = "kimne78kx3ncx6brgo4mv6wki5h1ko";

const twitchStreamSchema = Schema.Struct({
  title: Schema.String,
  viewersCount: Schema.Number,
  game: Schema.NullOr(Schema.Struct({ name: Schema.String })),
});

const twitchBroadcastSettingsSchema = Schema.Struct({
  liveUpNotification: Schema.NullOr(Schema.String),
});

const twitchGQLResponseSchema = Schema.Struct({
  data: Schema.Struct({
    user: Schema.NullOr(
      Schema.Struct({
        stream: Schema.NullOr(twitchStreamSchema),
        broadcastSettings: twitchBroadcastSettingsSchema,
      }),
    ),
  }),
});

type TwitchGQLResponse = Schema.Schema.Type<typeof twitchGQLResponseSchema>;

export function fetchTwitchLiveStatus({
  username,
}: {
  username: string;
}): Effect.Effect<FetchedStatus> {
  const query = `query{user(login:"${username}"){stream{title viewersCount game{name}}broadcastSettings{liveUpNotification}}}`;

  return fetchGQL(
    {
      url: TWITCH_GQL_URL,
      clientId: TWITCH_CLIENT_ID,
      query,
    },
    twitchGQLResponseSchema,
  ).pipe(
    Effect.map(extractLiveStatus),
    Effect.catchAll((error) =>
      Effect.succeed({
        status: LiveStatus.Unknown,
        error: error.message,
      } as FetchedStatus),
    ),
  );
}

export function extractLiveStatus(data: TwitchGQLResponse): FetchedStatus {
  const user = data.data.user;

  // User doesn't exist - this is a definitive "offline" (or non-existent)
  if (!user) {
    return { status: LiveStatus.Offline };
  }

  const stream = user.stream;
  if (!stream) {
    return { status: LiveStatus.Offline };
  }

  // Prefer liveUpNotification (custom notification message) over stream title
  const title = user.broadcastSettings.liveUpNotification || stream.title;

  return {
    status: LiveStatus.Live,
    title,
    viewerCount: stream.viewersCount,
    category: stream.game?.name,
  };
}

export function getTwitchLiveUrl(username: string): string {
  return `https://www.twitch.tv/${username}`;
}
