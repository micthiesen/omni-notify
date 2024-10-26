import { decode } from "html-entities";

const TIMEOUT_MS = 10 * 1000;

export type LiveStatusLive = { isLive: true; title: string; viewerCount?: number };
export type LiveStatusOffline = { isLive: false };
export type LiveStatus = LiveStatusLive | LiveStatusOffline;

export async function checkYouTubeLiveStatus({
	username,
}: { username: string }): Promise<LiveStatus> {
	const url = getYouTubeLiveUrl(username);
	const controller = new AbortController();
	const timeoutId = setTimeout(() => controller.abort(), TIMEOUT_MS);

	try {
		const response = await fetch(url, { signal: controller.signal });
		clearTimeout(timeoutId);

		if (!response.ok) {
			const body = await response.text();
			throw new Error(body);
		}

		const html = await response.text();
		return extractLiveStatus(html);
	} catch (error) {
		clearTimeout(timeoutId);
		throw new Error(
			`Failed to check YouTube live status: ${
				// biome-ignore lint/suspicious/noExplicitAny: <explanation>
				(error as any)?.message
			}`,
		);
	}
}

export function extractLiveStatus(html: string): LiveStatus {
	const metaTagRegex = /<meta\s+name="title"\s+content="([^"]*)"\s*\/?>/i;
	const match = metaTagRegex.exec(html);
	return match
		? { isLive: true, title: decode(match[1]), viewerCount: extractViewerCount(html) }
		: { isLive: false };
}

function extractViewerCount(html: string): number | undefined {
	const match = html.match(/(?<="text":")[\d,]+(?="})/);
	if (!match) return;

	const count = Number.parseInt(match[0].replace(/,/g, ""), 10);
	return Number.isNaN(count) ? undefined : count;
}

export function getYouTubeLiveUrl(username: string) {
	return `https://www.youtube.com/${username}/live`;
}
