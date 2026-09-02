import { extractFromHtml } from "@extractus/article-extractor";
import { Logger } from "@micthiesen/mitools/logging";
import { extractDomain } from "@micthiesen/mitools/strings";
import { Effect } from "effect";
import { PressPodsError, tryPromise } from "../effect.js";
import { cleanText } from "../formatting/index.js";
import { fetchPublicHtml } from "../publicHttp.js";

const LOGGER = Logger.named("PressPods.retrievers.extractus");

export function retrieveArticleExtractus(url: string, userAgent: string) {
  return fetchPublicHtml(url, userAgent).pipe(
    Effect.flatMap((html) =>
      tryPromise("parse article with Extractus", () => extractFromHtml(html, url)),
    ),
    Effect.flatMap((result) =>
      Effect.gen(function* () {
        if (!result?.content) {
          return yield* Effect.fail(
            new PressPodsError({
              operation: "parse article with Extractus",
              cause: new Error("Failed to extract article"),
            }),
          );
        }
        yield* LOGGER.debug("Parsed article with extractus:", result);
        return {
          title: result.title,
          text: cleanText(result.content),
          author: result.author,
          domain: extractDomain(url) ?? undefined,
          publishedAt: result.published ? new Date(result.published) : undefined,
          leadImageUrl: result.image ?? undefined,
          url,
        };
      }),
    ),
  );
}
