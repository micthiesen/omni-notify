import { extractDomain } from "@micthiesen/mitools/strings";
import got from "got";
import { cleanText } from "../formatting/index.js";
import type { Article } from "../types.js";
import { extractTitleFromHtml } from "./constants.js";

const WAYBACK_AVAILABILITY_API = "https://archive.org/wayback/available";

type WaybackResponse = {
  url: string;
  archived_snapshots: {
    closest?: {
      status: string;
      available: boolean;
      url: string;
      timestamp: string;
    };
  };
};

/**
 * Retrieve article from the Internet Archive Wayback Machine
 * (most recent archived snapshot of the URL).
 */
export async function retrieveArticleWayback(
  url: string,
  userAgent: string,
): Promise<Article> {
  const availabilityUrl = `${WAYBACK_AVAILABILITY_API}?url=${encodeURIComponent(url)}`;

  const availabilityResponse = await got<WaybackResponse>(availabilityUrl, {
    responseType: "json",
    timeout: { request: 10000 },
  });

  const snapshot = availabilityResponse.body.archived_snapshots.closest;
  if (!snapshot?.available) {
    throw new Error("No archived snapshot available for this URL");
  }

  const archivedResponse = await got(snapshot.url, {
    headers: { "User-Agent": userAgent },
    timeout: { request: 20000 },
    retry: { limit: 2, methods: ["GET"] },
  });

  const html = archivedResponse.body;

  return {
    title: extractTitleFromHtml(html),
    text: cleanText(html),
    author: undefined, // Archive doesn't provide author
    domain: extractDomain(url) ?? undefined,
    publishedAt: parseWaybackTimestamp(snapshot.timestamp),
    leadImageUrl: undefined,
    url,
  };
}

/** Parse Wayback Machine timestamp (YYYYMMDDhhmmss) to Date. */
function parseWaybackTimestamp(timestamp: string): Date | undefined {
  if (!timestamp || timestamp.length < 8) return undefined;

  const year = Number.parseInt(timestamp.slice(0, 4), 10);
  const month = Number.parseInt(timestamp.slice(4, 6), 10) - 1;
  const day = Number.parseInt(timestamp.slice(6, 8), 10);
  const hour = timestamp.length >= 10 ? Number.parseInt(timestamp.slice(8, 10), 10) : 0;
  const minute =
    timestamp.length >= 12 ? Number.parseInt(timestamp.slice(10, 12), 10) : 0;
  const second =
    timestamp.length >= 14 ? Number.parseInt(timestamp.slice(12, 14), 10) : 0;

  return new Date(year, month, day, hour, minute, second);
}
