import { extractDomain } from "@micthiesen/mitools/strings";
import got from "got";
import { cleanText } from "../formatting/index.js";
import type { Article } from "../types.js";
import { extractTitleFromHtml } from "./constants.js";

export async function retrieveArticleFetch(
  url: string,
  userAgent: string,
): Promise<Article> {
  const response = await got(url, {
    headers: { "User-Agent": userAgent },
    timeout: { request: 20000 },
    retry: { limit: 2, methods: ["GET"] },
  });
  const html = response.body;

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
