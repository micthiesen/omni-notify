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
	const metaTagRegex = /<meta\s+name="title"\s+content="([^"]*)"\s*\/?>/i;
	const match = metaTagRegex.exec(html);
	if (!match) return { isLive: false };

	return {
		isLive: true,
		title: decode(match[1]),
		viewerCount: extractViewerCount(html),
		debugContext: { metaTag: match[0] },
	};
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
