import { extractDomain } from "@micthiesen/mitools/strings";
import { Effect } from "effect";
import { PressPodsError } from "../effect.js";
import { cleanText } from "../formatting/index.js";
import { fetchPublicText } from "../publicHttp.js";
import type { Article } from "../types.js";
import { extractTitleFromHtml } from "./constants.js";

const REMOVEPAYWALL_BASE = "https://www.removepaywall.com";

/**
 * Retrieve article via the removepaywall.com proxy.
 * Useful for paywalled articles the other retrievers can't access.
 */
export function retrieveArticleRemovepaywall(
  url: string,
  _userAgent: string,
): Effect.Effect<Article, PressPodsError> {
  const proxyUrl = `${REMOVEPAYWALL_BASE}/search?url=${encodeURIComponent(url)}`;
  return fetchPublicText(
    proxyUrl,
    {
      timeout: { request: 30000 },
      retry: { limit: 2, methods: ["GET"] },
      followRedirect: true,
    },
    "retrieve article with removepaywall",
  ).pipe(
    Effect.flatMap((html) => {
      return html && html.length >= 100
        ? Effect.succeed({
            title: extractTitleFromHtml(html),
            text: cleanText(html),
            author: undefined,
            domain: extractDomain(url) ?? undefined,
            publishedAt: undefined,
            leadImageUrl: undefined,
            url,
          })
        : Effect.fail(
            new PressPodsError({
              operation: "retrieve article with removepaywall",
              cause: new Error("removepaywall returned empty or too short content"),
            }),
          );
    }),
  );
}
