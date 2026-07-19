import { extractDomain } from "@micthiesen/mitools/strings";
import { Readability } from "@mozilla/readability";
import got from "got";
import { parseHTML } from "linkedom";
import { cleanText } from "../formatting/index.js";
import type { Article } from "../types.js";

/**
 * Local extraction via Mozilla Readability (the Firefox Reader View
 * algorithm) — the same stack the AI fetchUrl tool uses, but feeding the
 * shared cleanText pipeline so quotes/prose match the other retrievers.
 */
export async function retrieveArticleReadability(
  url: string,
  userAgent: string,
): Promise<Article> {
  const html = await got(url, {
    headers: { "User-Agent": userAgent, Accept: "text/html" },
    timeout: { request: 20000 },
    retry: { limit: 2, methods: ["GET"] },
  }).text();

  const { document } = parseHTML(html);
  const leadImageUrl = document
    .querySelector('meta[property="og:image"]')
    ?.getAttribute("content");

  const article = new Readability(document as unknown as Document).parse();
  if (!article?.content) throw new Error("Readability could not parse the article");

  return {
    title: article.title ?? undefined,
    text: cleanText(article.content),
    author: article.byline ?? undefined,
    domain: extractDomain(url) ?? undefined,
    publishedAt: article.publishedTime ? new Date(article.publishedTime) : undefined,
    leadImageUrl: leadImageUrl ?? undefined,
    url,
  };
}
