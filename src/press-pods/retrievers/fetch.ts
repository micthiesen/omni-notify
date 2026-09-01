import { extractDomain } from "@micthiesen/mitools/strings";
import { Effect } from "effect";
import { PressPodsError } from "../effect.js";
import { cleanText } from "../formatting/index.js";
import { fetchPublicHtml } from "../publicHttp.js";
import type { Article } from "../types.js";
import { extractTitleFromHtml } from "./constants.js";

export function retrieveArticleFetch(
  url: string,
  userAgent: string,
): Effect.Effect<Article, PressPodsError> {
  return fetchPublicHtml(url, userAgent).pipe(
    Effect.map((html) => ({
      title: extractTitleFromHtml(html),
      text: cleanText(html),
      author: undefined,
      domain: extractDomain(url) ?? undefined,
      publishedAt: undefined,
      leadImageUrl: undefined,
      url,
    })),
  );
}
