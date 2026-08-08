import { decode } from "html-entities";
import { fetchPageHtml } from "./common.js";
import { type FetchedStatus, LiveStatus } from "./index.js";

export async function fetchYouTubeLiveStatus({
  username,
}: {
  username: string;
}): Promise<FetchedStatus> {
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
  const hasPlayerResponseAssignment = /\bytInitialPlayerResponse\s*=\s*/.test(html);
  const playerResponse = extractInitialPlayerResponse(html);
  if (!playerResponse) {
    // An offline channel's /live route renders the normal channel page. It has
    // ytInitialData and references ytInitialPlayerResponse in shared JS, but
    // does not assign a player response because there is no video to play.
    if (!hasPlayerResponseAssignment && /\bytInitialData\s*=\s*\{/.test(html)) {
      return { status: LiveStatus.Offline };
    }

    return {
      status: LiveStatus.Unknown,
      error: "Response missing expected YouTube data structure",
    };
  }

  const isLive =
    playerResponse.microformat?.playerMicroformatRenderer?.liveBroadcastDetails
      ?.isLiveNow === true;

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

interface YouTubePlayerResponse {
  microformat?: {
    playerMicroformatRenderer?: {
      liveBroadcastDetails?: {
        isLiveNow?: boolean;
      };
    };
  };
}

function extractInitialPlayerResponse(html: string): YouTubePlayerResponse | null {
  const assignment = /\bytInitialPlayerResponse\s*=\s*/g;

  for (const match of html.matchAll(assignment)) {
    const objectStart = (match.index ?? 0) + match[0].length;
    if (html[objectStart] !== "{") continue;

    const json = extractJsonObject(html, objectStart);
    if (!json) continue;

    try {
      const parsed: unknown = JSON.parse(json);
      if (typeof parsed === "object" && parsed !== null) {
        return parsed as YouTubePlayerResponse;
      }
    } catch {
      // Keep looking in case an earlier reference was not the player response assignment.
    }
  }

  return null;
}

function extractJsonObject(html: string, start: number): string | null {
  let depth = 0;
  let inString = false;
  let escaped = false;

  for (let index = start; index < html.length; index += 1) {
    const character = html[index];

    if (inString) {
      if (escaped) {
        escaped = false;
      } else if (character === "\\") {
        escaped = true;
      } else if (character === '"') {
        inString = false;
      }
      continue;
    }

    if (character === '"') {
      inString = true;
    } else if (character === "{") {
      depth += 1;
    } else if (character === "}") {
      depth -= 1;
      if (depth === 0) return html.slice(start, index + 1);
    }
  }

  return null;
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
