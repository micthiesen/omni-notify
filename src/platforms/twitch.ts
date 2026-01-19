import { z } from "zod";
import { fetchGQL } from "./common.js";
import type { FetchedStatus } from "./index.js";

const TWITCH_GQL_URL = "https://gql.twitch.tv/gql";

// Public client ID used by Twitch's web player and many open source projects.
// No authentication required. See: https://github.com/nicknsy/twitch-api/wiki/Public-GraphQL-queries
const TWITCH_CLIENT_ID = "kimne78kx3ncx6brgo4mv6wki5h1ko";

const twitchGQLResponseSchema = z.object({
	data: z.object({
		user: z
			.object({
				stream: z
					.object({
						title: z.string(),
						viewersCount: z.number(),
					})
					.nullable(),
			})
			.nullable(),
	}),
});

type TwitchGQLResponse = z.infer<typeof twitchGQLResponseSchema>;

export async function fetchTwitchLiveStatus({
	username,
}: { username: string }): Promise<FetchedStatus> {
	const query = `query{user(login:"${username}"){stream{title viewersCount}}}`;
	const raw = await fetchGQL<unknown>({
		url: TWITCH_GQL_URL,
		clientId: TWITCH_CLIENT_ID,
		query,
	});

	const result = twitchGQLResponseSchema.safeParse(raw);
	if (!result.success) {
		return { isLive: false, debugContext: { error: result.error.message, raw } };
	}

	return extractLiveStatus(result.data);
}

export function extractLiveStatus(data: TwitchGQLResponse): FetchedStatus {
	const stream = data.data?.user?.stream;
	if (!stream) return { isLive: false };

	return {
		isLive: true,
		title: stream.title,
		viewerCount: stream.viewersCount,
	};
}

export function getTwitchLiveUrl(username: string): string {
	return `https://www.twitch.tv/${username}`;
}
