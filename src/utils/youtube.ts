import { decode } from "html-entities";

const TIMEOUT_MS = 10 * 1000;

export type IsLiveResult = { isLive: true; title: string } | { isLive: false };

export async function checkYouTubeLiveStatus({
	username,
}: { username: string }): Promise<IsLiveResult> {
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
		return extractMetaTitleContent(html);
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

export function extractMetaTitleContent(html: string): IsLiveResult {
	const metaTagRegex = /<meta\s+name="title"\s+content="([^"]*)"\s*\/?>/i;
	const match = metaTagRegex.exec(html);
	return match ? { isLive: true, title: decode(match[1]) } : { isLive: false };
}

export function getYouTubeLiveUrl(username: string) {
	return `https://www.youtube.com/${username}/live`;
}
