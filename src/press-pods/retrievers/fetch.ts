import { extractDomain } from "@micthiesen/mitools/strings";
import { cleanText } from "../formatting/index.js";
import { fetchPublicHtml } from "../publicHttp.js";
import type { Article } from "../types.js";
import { extractTitleFromHtml } from "./constants.js";

export async function retrieveArticleFetch(
  url: string,
  userAgent: string,
): Promise<Article> {
  const html = await fetchPublicHtml(url, userAgent);

  return {
    title: extractTitleFromHtml(html),
    text: cleanText(html),
    author: undefined,
    domain: extractDomain(url) ?? undefined,
    publishedAt: undefined,
    leadImageUrl: undefined,
    url,
  };
}
