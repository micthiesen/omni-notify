import { extractDomain } from "@micthiesen/mitools/strings";
import { Readability } from "@mozilla/readability";
import { parseHTML } from "linkedom";
import { Effect } from "effect";
import { PressPodsError, trySync } from "../effect.js";
import { cleanText } from "../formatting/index.js";
import { fetchPublicHtml } from "../publicHttp.js";
import type { Article } from "../types.js";

/**
 * Local extraction via Mozilla Readability (the Firefox Reader View
 * algorithm) — the same stack the AI fetchUrl tool uses, but feeding the
 * shared cleanText pipeline so quotes/prose match the other retrievers.
 */
export function retrieveArticleReadability(
  url: string,
  userAgent: string,
): Effect.Effect<Article, PressPodsError> {
  return fetchPublicHtml(url, userAgent).pipe(
    Effect.flatMap((html) =>
      trySync("parse article with Readability", () => {
        const { document } = parseHTML(html);
        const leadImageUrl = document
          .querySelector('meta[property="og:image"]')
          ?.getAttribute("content");
        const article = new Readability(document as unknown as Document).parse();
        if (!article?.content) {
          throw new Error("Readability could not parse the article");
        }
        return {
          title: article.title ?? undefined,
          text: cleanText(article.content),
          author: article.byline ?? undefined,
          domain: extractDomain(url) ?? undefined,
          publishedAt: article.publishedTime
            ? new Date(article.publishedTime)
            : undefined,
          leadImageUrl: leadImageUrl ?? undefined,
          url,
        };
      }),
    ),
  );
}
