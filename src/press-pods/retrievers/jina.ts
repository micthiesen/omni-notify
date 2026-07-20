import { extractDomain } from "@micthiesen/mitools/strings";
import got from "got";
import { currentCostFeature, recordCostEventSafely } from "../../costs/persistence.js";
import config from "../../utils/config.js";
import { cleanText } from "../formatting/index.js";
import type { Article } from "../types.js";
import { extractTitleFromHtml } from "./constants.js";

const JINA_API_BASE = "https://r.jina.ai";

/**
 * Retrieve article using Jina.ai Reader API.
 * Uses a headless browser to render JS-heavy pages.
 */
export async function retrieveArticleJina(
  url: string,
  _userAgent: string,
): Promise<Article> {
  const jinaUrl = `${JINA_API_BASE}/${url}`;

  const response = await got(jinaUrl, {
    headers: {
      Authorization: `Bearer ${config.JINA_API_KEY}`,
      "X-Return-Format": "html",
      "X-Target-Selector":
        "article, main, [role=main], .article-body, .post-content, .entry-content",
      "X-Remove-Selector":
        "nav, footer, header, aside, .sidebar, .comments, .related, .social-share, .advertisement, [role=navigation], [role=banner], [role=contentinfo]",
    },
    timeout: { request: 30000 },
    retry: { limit: 2, methods: ["GET"] },
  });

  const html = response.body;
  if (!html || html.length < 100) {
    throw new Error("Jina returned empty or too short content");
  }

  recordCostEventSafely({
    category: "retrieval",
    feature: currentCostFeature("press-pods"),
    operation: "retrieve-article",
    service: "jina",
    model: "reader",
    costCents: null,
    priceStatus: "unknown",
    usage: { requests: 1 },
  });

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
