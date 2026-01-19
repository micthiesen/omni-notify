import { decode } from "html-entities";
import { fetchPageHtml } from "./common.js";
import { type FetchedStatus, LiveStatus } from "./index.js";

export async function fetchYouTubeLiveStatus({
	username,
}: { username: string }): Promise<FetchedStatus> {
	const url = getYouTubeLiveUrl(username);

	let html: string;
	try {
		html = await fetchPageHtml(url);
	} catch (error) {
		return {
			status: LiveStatus.Unknown,
			error: error instanceof Error ? error.message : String(error),
		};
	}

	return extractLiveStatus(html);
}

export function extractLiveStatus(html: string): FetchedStatus {
	// Check for signs that we got a non-standard response (CAPTCHA, consent, etc.)
	if (!html.includes("ytInitialPlayerResponse")) {
		return {
			status: LiveStatus.Unknown,
			error: "Response missing expected YouTube data structure",
		};
	}

	const isLive =
		/"isLive"\s*:\s*true/i.test(html) || /"isLiveNow"\s*:\s*true/i.test(html);

	if (!isLive) {
		return { status: LiveStatus.Offline };
	}

	const title = extractTitle(html);
	if (!title) {
		return {
			status: LiveStatus.Unknown,
			error: "Live detected but failed to extract title",
		};
	}

	return {
		status: LiveStatus.Live,
		title,
		viewerCount: extractViewerCount(html),
	};
}

function extractTitle(html: string): string | null {
	const metaTagRegex = /<meta\s+name="title"\s+content="([^"]*)"\s*\/?>/i;
	const match = metaTagRegex.exec(html);
	return match ? decode(match[1]) : null;
}

function extractViewerCount(html: string): number | undefined {
	const match = html.match(/(?<="viewCount":{"runs":\[{"text":")[\d,]+(?="})/);
	if (!match) return undefined;

	const count = Number.parseInt(match[0].replace(/,/g, ""), 10);
	return Number.isNaN(count) ? undefined : count;
}

export function getYouTubeLiveUrl(username: string): string {
	return `https://www.youtube.com/${username}/live`;
}
