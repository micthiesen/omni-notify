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
  const allResults = await Promise.all(
    getArticleRetrievers().map((retriever) =>
      withTimeout(
        tryGetArticleFromUrl(url, retriever, costCounter),
        RETRIEVER_TIMEOUT_MS,
      ).catch((error) => ({
        success: false as const,
        error,
        retrieverName: retriever.name,
      })),
    ),
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

async function tryGetArticleFromUrl(
  url: string,
  retriever: ArticleRetriever,
  costCounter: CostCounter,
): Promise<ArticleRetrieverResult> {
  try {
    const article = await retriever.retrieve(url, USER_AGENT);
    const metadata = await getArticleMetadata(article, costCounter);
    if (metadata.info.isValidArticle) {
      return { success: true, article, metadata, retrieverName: retriever.name };
    }

    return {
      success: false,
      error: new Error("Invalid article"),
      retrieverName: retriever.name,
    };
  } catch (error) {
    return { success: false, error, retrieverName: retriever.name };
  }
}
