import { extractDomain } from "@micthiesen/mitools/strings";
import got from "got";
import { cleanText } from "../formatting/index.js";
import type { Article } from "../types.js";
import { extractTitleFromHtml } from "./constants.js";

const REMOVEPAYWALL_BASE = "https://www.removepaywall.com";

/**
 * Retrieve article via the removepaywall.com proxy.
 * Useful for paywalled articles the other retrievers can't access.
 */
export async function retrieveArticleRemovepaywall(
  url: string,
  _userAgent: string,
): Promise<Article> {
  const proxyUrl = `${REMOVEPAYWALL_BASE}/search?url=${encodeURIComponent(url)}`;

  const response = await got(proxyUrl, {
    timeout: { request: 30000 },
    retry: { limit: 2, methods: ["GET"] },
    followRedirect: true,
  });

  const html = response.body;
  if (!html || html.length < 100) {
    throw new Error("removepaywall returned empty or too short content");
  }

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
