import { extract } from "@extractus/article-extractor";
import { Logger } from "@micthiesen/mitools/logging";
import { extractDomain } from "@micthiesen/mitools/strings";
import { cleanText } from "../formatting/index.js";
import type { Article } from "../types.js";

const LOGGER = new Logger("PressPods.retrievers.extractus");

export async function retrieveArticleExtractus(
  url: string,
  userAgent: string,
): Promise<Article> {
  const result = await extract(url, {}, { headers: { "User-Agent": userAgent } });
  if (!result?.content) throw new Error("Failed to extract article");
  LOGGER.debug("Parsed article with extractus:", result);

  return {
    title: result.title,
    text: cleanText(result.content),
    author: result.author,
    domain: extractDomain(url) ?? undefined,
    publishedAt: result.published ? new Date(result.published) : undefined,
    leadImageUrl: result.image ?? undefined,
    url,
  };
}
