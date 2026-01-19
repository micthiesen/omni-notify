import { z } from "zod";
import { fetchGQL } from "./common.js";
import { type FetchedStatus, LiveStatus } from "./index.js";

const TWITCH_GQL_URL = "https://gql.twitch.tv/gql";

// Public client ID used by Twitch's web player and many open source projects.
// No authentication required. See: https://github.com/nicknsy/twitch-api/wiki/Public-GraphQL-queries
const TWITCH_CLIENT_ID = "kimne78kx3ncx6brgo4mv6wki5h1ko";

const twitchStreamSchema = z.object({
  title: z.string(),
  viewersCount: z.number(),
});

const twitchGQLResponseSchema = z.object({
  data: z.object({
    user: z
      .object({
        stream: twitchStreamSchema.nullable(),
      })
      .nullable(),
  }),
});

type TwitchGQLResponse = z.infer<typeof twitchGQLResponseSchema>;

export async function fetchTwitchLiveStatus({
  username,
}: { username: string }): Promise<FetchedStatus> {
  const query = `query{user(login:"${username}"){stream{title viewersCount}}}`;

  let raw: unknown;
  try {
    raw = await fetchGQL<unknown>({
      url: TWITCH_GQL_URL,
      clientId: TWITCH_CLIENT_ID,
      query,
    });
  } catch (error) {
    return {
      status: LiveStatus.Unknown,
      error: error instanceof Error ? error.message : String(error),
    };
  }

  const result = twitchGQLResponseSchema.safeParse(raw);
  if (!result.success) {
    return {
      status: LiveStatus.Unknown,
      error: `Invalid API response: ${result.error.message}`,
    };
  }

  return extractLiveStatus(result.data);
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

  return {
    status: LiveStatus.Live,
    title: stream.title,
    viewerCount: stream.viewersCount,
  };
}

export function getTwitchLiveUrl(username: string): string {
  return `https://www.twitch.tv/${username}`;
}
