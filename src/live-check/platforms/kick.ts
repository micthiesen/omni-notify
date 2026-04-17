import got from "got";
import { z } from "zod";
import { type FetchedStatus, LiveStatus } from "./index.js";

const TIMEOUT_MS = 10_000;
const TOKEN_URL = "https://id.kick.com/oauth/token";
const CHANNELS_URL = "https://api.kick.com/public/v1/channels";

// Refresh proactively this many ms before the token's stated expiry.
const TOKEN_REFRESH_LEEWAY_MS = 60_000;

const tokenResponseSchema = z.object({
  access_token: z.string(),
  token_type: z.string(),
  expires_in: z.number(),
});

type CachedToken = { accessToken: string; expiresAt: number };
let cachedToken: CachedToken | null = null;

async function fetchAccessToken(): Promise<string> {
  const clientId = process.env.KICK_CLIENT_ID;
  const clientSecret = process.env.KICK_CLIENT_SECRET;
  if (!clientId || !clientSecret) {
    throw new Error(
      "KICK_CLIENT_ID and KICK_CLIENT_SECRET env vars are required for Kick",
    );
  }

  const raw = await got
    .post(TOKEN_URL, {
      form: {
        grant_type: "client_credentials",
        client_id: clientId,
        client_secret: clientSecret,
      },
      timeout: { request: TIMEOUT_MS },
    })
    .json<unknown>();

  const parsed = tokenResponseSchema.parse(raw);
  cachedToken = {
    accessToken: parsed.access_token,
    expiresAt: Date.now() + parsed.expires_in * 1000 - TOKEN_REFRESH_LEEWAY_MS,
  };
  return parsed.access_token;
}

async function getAccessToken(): Promise<string> {
  if (cachedToken && cachedToken.expiresAt > Date.now()) {
    return cachedToken.accessToken;
  }
  return fetchAccessToken();
}

const categorySchema = z
  .object({ id: z.number(), name: z.string() })
  .partial()
  .nullable();

const streamSchema = z
  .object({
    is_live: z.boolean(),
    viewer_count: z.number().optional(),
    start_time: z.string().optional(),
  })
  .nullable();

const kickChannelSchema = z.object({
  slug: z.string(),
  stream_title: z.string().optional().default(""),
  category: categorySchema.optional(),
  stream: streamSchema.optional(),
});

const kickChannelsResponseSchema = z.object({
  data: z.array(kickChannelSchema),
  message: z.string().optional(),
});

export type KickChannelsResponse = z.infer<typeof kickChannelsResponseSchema>;

export async function fetchKickLiveStatus({
  username,
}: {
  username: string;
}): Promise<FetchedStatus> {
  const request = async (bearer: string) =>
    got(CHANNELS_URL, {
      searchParams: { slug: username },
      headers: { Authorization: `Bearer ${bearer}` },
      timeout: { request: TIMEOUT_MS },
      throwHttpErrors: false,
      responseType: "json",
    });

  let raw: unknown;
  try {
    let token = await getAccessToken();
    let response = await request(token);

    // Token may have been revoked or rotated — force one refresh and retry.
    if (response.statusCode === 401) {
      cachedToken = null;
      token = await getAccessToken();
      response = await request(token);
    }

    if (response.statusCode < 200 || response.statusCode >= 300) {
      const snippet = JSON.stringify(response.body).slice(0, 200);
      return {
        status: LiveStatus.Unknown,
        error: `Kick API returned ${response.statusCode}: ${snippet}`,
      };
    }

    raw = response.body;
  } catch (error) {
    return {
      status: LiveStatus.Unknown,
      error: error instanceof Error ? error.message : String(error),
    };
  }

  const result = kickChannelsResponseSchema.safeParse(raw);
  if (!result.success) {
    return {
      status: LiveStatus.Unknown,
      error: `Invalid Kick API response: ${result.error.message}`,
    };
  }

  return extractLiveStatus(result.data);
}

export function extractLiveStatus(data: KickChannelsResponse): FetchedStatus {
  const channel = data.data[0];
  if (!channel) {
    return { status: LiveStatus.Offline };
  }
  if (!channel.stream?.is_live) {
    return { status: LiveStatus.Offline };
  }
  return {
    status: LiveStatus.Live,
    title: channel.stream_title || channel.slug,
    viewerCount: channel.stream.viewer_count,
    category: channel.category?.name,
  };
}

export function getKickLiveUrl(username: string): string {
  return `https://kick.com/${username}`;
}
