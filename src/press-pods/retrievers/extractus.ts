import { extractFromHtml } from "@extractus/article-extractor";
import { Logger } from "@micthiesen/mitools/logging";
import { extractDomain } from "@micthiesen/mitools/strings";
import { Effect } from "effect";
import { PressPodsError, tryPromise } from "../effect.js";
import { cleanText } from "../formatting/index.js";
import { fetchPublicHtml } from "../publicHttp.js";
import type { Article } from "../types.js";

const LOGGER = new Logger("PressPods.retrievers.extractus");

export function retrieveArticleExtractus(
  url: string,
  userAgent: string,
): Effect.Effect<Article, PressPodsError> {
  return fetchPublicHtml(url, userAgent).pipe(
    Effect.flatMap((html) =>
      tryPromise("parse article with Extractus", () => extractFromHtml(html, url)),
    ),
    Effect.flatMap((result) => {
      if (!result?.content) {
        return Effect.fail(
          new PressPodsError({
            operation: "parse article with Extractus",
            cause: new Error("Failed to extract article"),
          }),
        );
      }
      LOGGER.debug("Parsed article with extractus:", result);
      return Effect.succeed({
        title: result.title,
        text: cleanText(result.content),
        author: result.author,
        domain: extractDomain(url) ?? undefined,
        publishedAt: result.published ? new Date(result.published) : undefined,
        leadImageUrl: result.image ?? undefined,
        url,
      });
    }),
  );
}
