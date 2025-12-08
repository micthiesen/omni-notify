import { decode } from "html-entities";
import { fetchPageHtml } from "./common.js";
import type { FetchedStatus } from "./index.js";

export async function fetchYouTubeLiveStatus({
	username,
}: { username: string }): Promise<FetchedStatus> {
	const url = getYouTubeLiveUrl(username);
	const html = await fetchPageHtml(url);
	return extractLiveStatus(html);
}

export function extractLiveStatus(html: string): FetchedStatus {
	const isLiveMatch =
		/"isLive"\s*:\s*true/i.test(html) || /"isLiveNow"\s*:\s*true/i.test(html);

	if (!isLiveMatch) return { isLive: false };

	// Extract title from meta tag
	const metaTagRegex = /<meta\s+name="title"\s+content="([^"]*)"\s*\/?>/i;
	const titleMatch = metaTagRegex.exec(html);
	const title = titleMatch ? decode(titleMatch[1]) : "Unknown Title";

	return { isLive: true, title, viewerCount: extractViewerCount(html) };
}

function extractViewerCount(html: string): number | undefined {
	const match = html.match(/(?<="viewCount":{"runs":\[{"text":")[\d,]+(?="})/);
	if (!match) return;

	const count = Number.parseInt(match[0].replace(/,/g, ""), 10);
	return Number.isNaN(count) ? undefined : count;
}

export function getYouTubeLiveUrl(username: string) {
	return `https://www.youtube.com/${username}/live`;
}
