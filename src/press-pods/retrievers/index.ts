import { extractHttpError } from "@micthiesen/mitools/http";
import type { Logger } from "@micthiesen/mitools/logging";
import { Duration, Effect, Either } from "effect";
import config from "../../utils/config.js";
import { getArticleMetadataEffect, type Metadata } from "../agents/metadata.js";
import type CostCounter from "../costs.js";
import { assertPublicHttpUrl } from "../publicHttp.js";
import type { Article, ArticleRetriever, ArticleRetrieverResult } from "../types.js";
import { USER_AGENT } from "./constants.js";
import { retrieveArticleExtractus } from "./extractus.js";
import { retrieveArticleFetch } from "./fetch.js";
import { retrieveArticleJina } from "./jina.js";
import { retrieveArticlePostlight } from "./postlight.js";
import { retrieveArticleReadability } from "./readability.js";
import { retrieveArticleRemovepaywall } from "./removepaywall.js";
import { retrieveArticleWayback } from "./wayback.js";
import { retrieveArticleX } from "./x.js";
import { PressPodsError } from "../effect.js";

const RETRIEVER_TIMEOUT_MS = 60_000;

type RetrievedArticleResult =
  | { success: false; error: unknown; retrieverName: string }
  | { success: true; article: Article; retrieverName: string };

export function runArticleRetrievers(
  url: string,
  retrievers: ArticleRetriever[],
  timeoutMs = RETRIEVER_TIMEOUT_MS,
): Effect.Effect<RetrievedArticleResult[]> {
  return Effect.forEach(
    retrievers,
    (retriever) =>
      retrieveArticle(url, retriever).pipe(
        Effect.timeout(Duration.millis(timeoutMs)),
        Effect.either,
        Effect.map((result): RetrievedArticleResult =>
          Either.isRight(result)
            ? result.right
            : { success: false, error: result.left, retrieverName: retriever.name },
        ),
      ),
    { concurrency: "unbounded" },
  );
}

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
export function rateRetrievedArticles(
  retrieved: RetrievedArticleResult[],
  rateArticle: (article: Article) => Effect.Effect<Metadata, PressPodsError>,
): Effect.Effect<ArticleRetrieverResult[]> {
  return Effect.gen(function* () {
    const results = Array<ArticleRetrieverResult>(retrieved.length);
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

    yield* Effect.forEach(
      [...groups.values()],
      (group) =>
        rateArticle(group[0].article).pipe(
          Effect.either,
          Effect.map((rated) => {
            if (Either.isRight(rated)) {
              const metadata = rated.right;
              for (const member of group) {
                const retrieverName = retrieved[member.index].retrieverName;
                results[member.index] = metadata.info.isValidArticle
                  ? { success: true, article: member.article, metadata, retrieverName }
                  : {
                      success: false,
                      error: new Error("Invalid article"),
                      retrieverName,
                    };
              }
            } else {
              const error = rated.left.cause;
              for (const member of group) {
                results[member.index] = {
                  success: false,
                  error,
                  retrieverName: retrieved[member.index].retrieverName,
                };
              }
            }
          }),
        ),
      { concurrency: 4, discard: true },
    );

    return results;
  });
}

function isXStatusUrl(rawUrl: string): boolean {
  try {
    const url = new URL(rawUrl);
    const hostname = url.hostname.toLowerCase().replace(/^www\./, "");
    return (
      ["x.com", "mobile.x.com", "twitter.com", "mobile.twitter.com"].includes(
        hostname,
      ) && /^\/(?:[^/]+\/status|i\/web\/status)\/\d+/.test(url.pathname)
    );
  } catch {
    return false;
  }
}

export function getArticleRetrievers(url?: string): ArticleRetriever[] {
  if (url && isXStatusUrl(url)) {
    return [{ name: "x", retrieve: retrieveArticleX }];
  }

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
export function getArticleFromUrl(
  url: string,
  costCounter: CostCounter,
  logger: Logger,
): Effect.Effect<
  {
    article: Article;
    metadata: Metadata;
    retrieverName: string;
    allResults: ArticleRetrieverResult[];
  },
  PressPodsError
> {
  return Effect.gen(function* () {
    yield* assertPublicHttpUrl(url);
    const retrieved = yield* runArticleRetrievers(url, getArticleRetrievers(url));
    const allResults = yield* rateRetrievedArticles(retrieved, (article) =>
      getArticleMetadataEffect(article, costCounter).pipe(
        Effect.timeout(Duration.millis(RETRIEVER_TIMEOUT_MS)),
        Effect.mapError((cause) =>
          cause instanceof PressPodsError
            ? cause
            : new PressPodsError({
                operation: "rate retrieved PressPods article",
                cause,
              }),
        ),
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
      return yield* new PressPodsError({
        operation: "retrieve PressPods article",
        cause: new Error("All article retrievers failed"),
      });
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
  });
}

function retrieveArticle(
  url: string,
  retriever: ArticleRetriever,
): Effect.Effect<RetrievedArticleResult, PressPodsError> {
  return Effect.suspend(() => retriever.retrieve(url, USER_AGENT)).pipe(
    Effect.map((article) => ({
      success: true,
      article,
      retrieverName: retriever.name,
    })),
  );
}
