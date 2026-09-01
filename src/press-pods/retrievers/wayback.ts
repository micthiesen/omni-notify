import { extractDomain } from "@micthiesen/mitools/strings";
import { Effect, Schema } from "effect";
import { PressPodsError } from "../effect.js";
import { cleanText } from "../formatting/index.js";
import { fetchPublicJson, fetchPublicText } from "../publicHttp.js";
import type { Article } from "../types.js";
import { extractTitleFromHtml } from "./constants.js";

const WAYBACK_AVAILABILITY_API = "https://archive.org/wayback/available";

const WaybackResponseSchema = Schema.Struct({
  url: Schema.String,
  archived_snapshots: Schema.Struct({
    closest: Schema.optional(
      Schema.Struct({
        status: Schema.String,
        available: Schema.Boolean,
        url: Schema.String,
        timestamp: Schema.String,
      }),
    ),
  }),
});

/**
 * Retrieve article from the Internet Archive Wayback Machine
 * (most recent archived snapshot of the URL).
 */
export function retrieveArticleWayback(
  url: string,
  userAgent: string,
): Effect.Effect<Article, PressPodsError> {
  const availabilityUrl = `${WAYBACK_AVAILABILITY_API}?url=${encodeURIComponent(url)}`;
  return fetchPublicJson(
    availabilityUrl,
    { timeout: { request: 10000 } },
    "query Wayback availability",
  ).pipe(
    Effect.flatMap((raw) =>
      Schema.decodeUnknown(WaybackResponseSchema)(raw).pipe(
        Effect.mapError(
          (cause) =>
            new PressPodsError({ operation: "decode Wayback response", cause }),
        ),
      ),
    ),
    Effect.flatMap((availabilityResponse) => {
      const snapshot = availabilityResponse.archived_snapshots.closest;
      if (!snapshot?.available) {
        return Effect.fail(
          new PressPodsError({
            operation: "retrieve article with Wayback",
            cause: new Error("No archived snapshot available for this URL"),
          }),
        );
      }
      return fetchPublicText(
        snapshot.url,
        {
          headers: { "User-Agent": userAgent },
          timeout: { request: 20000 },
          retry: { limit: 2, methods: ["GET"] },
        },
        "fetch Wayback snapshot",
      ).pipe(
        Effect.map((html) => {
          return {
            title: extractTitleFromHtml(html),
            text: cleanText(html),
            author: undefined,
            domain: extractDomain(url) ?? undefined,
            publishedAt: parseWaybackTimestamp(snapshot.timestamp),
            leadImageUrl: undefined,
            url,
          };
        }),
      );
    }),
  );
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
