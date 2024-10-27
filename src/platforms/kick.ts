import { fetchPageHtml } from "./common.js";
import type { FetchedStatus } from "./index.js";

export async function fetchKickLiveStatus({
	username,
}: { username: string }): Promise<FetchedStatus> {
	const url = getKickLiveUrl(username);
	const html = await fetchPageHtml(url);
	return extractLiveStatus(html);
}

export function extractLiveStatus(html: string): FetchedStatus {
	const eventRegex = /{"@type":"Event","name":"([^"]+)",/;
	const match = eventRegex.exec(html);
	return match
		? { isLive: true, title: match[1], viewerCount: extractViewerCount(html) }
		: { isLive: false };
}

function extractViewerCount(html: string): number | undefined {
	const match = html.match(/\\"viewer_count\\":(\d+),/);
	if (!match) return;

	const count = Number.parseInt(match[1], 10);
	return Number.isNaN(count) ? undefined : count;
}

export function getKickLiveUrl(username: string) {
	return `https://kick.com/${username}`;
}
