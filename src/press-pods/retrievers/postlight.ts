import { Logger } from "@micthiesen/mitools/logging";
import Parser from "@postlight/parser";
import { cleanText } from "../formatting/index.js";
import type { Article } from "../types.js";

const LOGGER = new Logger("PressPods.retrievers.postlight");

export async function retrieveArticlePostlight(
  url: string,
  userAgent: string,
): Promise<Article> {
  const result = await Parser.parse(url, {
    contentType: "html",
    headers: { "User-Agent": userAgent },
  });
  LOGGER.debug("Parsed article with postlight:", result);

  return {
    title: result.title ?? undefined,
    text: cleanText(result.content ?? ""),
    author: result.author ?? undefined,
    domain: result.domain ?? undefined,
    publishedAt: result.date_published ? new Date(result.date_published) : undefined,
    leadImageUrl: result.lead_image_url ?? undefined,
    url,
  };
}
