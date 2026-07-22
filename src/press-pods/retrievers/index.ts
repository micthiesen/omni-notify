import { withTimeout } from "@micthiesen/mitools/async";
import { extractHttpError } from "@micthiesen/mitools/http";
import type { Logger } from "@micthiesen/mitools/logging";
import config from "../../utils/config.js";
import { getArticleMetadata, type Metadata } from "../agents/metadata.js";
import type CostCounter from "../costs.js";
import type { Article, ArticleRetriever, ArticleRetrieverResult } from "../types.js";
import { USER_AGENT } from "./constants.js";
import { retrieveArticleExtractus } from "./extractus.js";
import { retrieveArticleFetch } from "./fetch.js";
import { retrieveArticleJina } from "./jina.js";
import { retrieveArticlePostlight } from "./postlight.js";
import { retrieveArticleReadability } from "./readability.js";
import { retrieveArticleRemovepaywall } from "./removepaywall.js";
import { retrieveArticleWayback } from "./wayback.js";

const RETRIEVER_TIMEOUT_MS = 60_000;

type RetrievedArticleResult =
  | { success: false; error: unknown; retrieverName: string }
  | { success: true; article: Article; retrieverName: string };

function articleRatingFingerprint(article: Article): string {
  return JSON.stringify({
    title: article.title ?? null,
    text: article.text.replace(/\r\n?/g, "\n").trim(),
    author: article.author ?? null,
    domain: article.domain ?? null,
    url: article.url,
    publishedAt: article.publishedAt?.toISOString() ?? null,
    leadImageUrl: article.leadImageUrl ?? null,
  });
}

/**
 * Rate each distinct extraction once, then fan that result back out to every
 * retriever that produced the same article text. The returned array preserves
 * provider order and still has exactly one result per provider.
 */
export async function rateRetrievedArticles(
  retrieved: RetrievedArticleResult[],
  rateArticle: (article: Article) => Promise<Metadata>,
): Promise<ArticleRetrieverResult[]> {
  const results = new Array<ArticleRetrieverResult>(retrieved.length);
  const groups = new Map<string, Array<{ index: number; article: Article }>>();

  for (const [index, result] of retrieved.entries()) {
    if (!result.success) {
      results[index] = result;
      continue;
    }
    const key = articleRatingFingerprint(result.article);
    const group = groups.get(key) ?? [];
    group.push({ index, article: result.article });
    groups.set(key, group);
  }

  await Promise.all(
    [...groups.values()].map(async (group) => {
      try {
        const metadata = await rateArticle(group[0].article);
        for (const member of group) {
          const retrieverName = retrieved[member.index].retrieverName;
          results[member.index] = metadata.info.isValidArticle
            ? { success: true, article: member.article, metadata, retrieverName }
            : { success: false, error: new Error("Invalid article"), retrieverName };
        }
      } catch (error) {
        for (const member of group) {
          results[member.index] = {
            success: false,
            error,
            retrieverName: retrieved[member.index].retrieverName,
          };
        }
      }
    }),
  );

  return results;
}

export function getArticleRetrievers(): ArticleRetriever[] {
  const retrievers: ArticleRetriever[] = [
    { name: "postlight", retrieve: retrieveArticlePostlight },
    { name: "readability", retrieve: retrieveArticleReadability },
    { name: "extractus", retrieve: retrieveArticleExtractus },
    { name: "wayback", retrieve: retrieveArticleWayback },
    { name: "removepaywall", retrieve: retrieveArticleRemovepaywall },
    { name: "fetch", retrieve: retrieveArticleFetch },
  ];
  if (config.JINA_API_KEY) {
    retrievers.push({ name: "jina", retrieve: retrieveArticleJina });
  }
  return retrievers;
}

/**
 * Run every retriever in parallel, have the metadata model rate each result's
 * extraction quality (0-10), and pick the best. One bad retriever can never
 * hurt the outcome; it just loses the rating contest.
 */
export async function getArticleFromUrl(
  url: string,
  costCounter: CostCounter,
  logger: Logger,
): Promise<{
  article: Article;
  metadata: Metadata;
  retrieverName: string;
  allResults: ArticleRetrieverResult[];
}> {
  const retrieved = await Promise.all(
    getArticleRetrievers().map((retriever) =>
      withTimeout(retrieveArticle(url, retriever), RETRIEVER_TIMEOUT_MS).catch(
        (error) => ({
          success: false as const,
          error,
          retrieverName: retriever.name,
        }),
      ),
    ),
  );
  const allResults = await rateRetrievedArticles(retrieved, (article) =>
    withTimeout(getArticleMetadata(article, costCounter), RETRIEVER_TIMEOUT_MS),
  );
  const successResults = allResults.filter((result) => result.success);
  if (successResults.length === 0) {
    for (const result of allResults) {
      if (result.success) continue;
      logger.warn(
        `Retriever ${result.retrieverName} failed:`,
        extractHttpError(result.error),
        true,
      );
    }
    throw new Error("All article retrievers failed");
  }

  const bestResult = successResults.sort(
    (a, b) => b.metadata.info.contentRating - a.metadata.info.contentRating,
  )[0];
  logger.info(`Retriever ${bestResult.retrieverName} selected as best`);

  return {
    article: bestResult.article,
    metadata: bestResult.metadata,
    retrieverName: bestResult.retrieverName,
    allResults,
  };
}

async function retrieveArticle(
  url: string,
  retriever: ArticleRetriever,
): Promise<RetrievedArticleResult> {
  try {
    const article = await retriever.retrieve(url, USER_AGENT);
    return { success: true, article, retrieverName: retriever.name };
  } catch (error) {
    return { success: false, error, retrieverName: retriever.name };
  }
}
