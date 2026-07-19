import type { Metadata } from "./agents/metadata.js";

export interface Article {
  title: string | undefined;
  text: string;
  author: string | undefined;
  domain: string | undefined;
  url: string;
  publishedAt: Date | undefined;
  leadImageUrl: string | undefined;
}

export interface ArticleRetriever {
  name: string;
  retrieve: (url: string, userAgent: string) => Promise<Article>;
}

export type ArticleRetrieverResult =
  | { success: false; error: unknown; retrieverName: string }
  | { success: true; article: Article; metadata: Metadata; retrieverName: string };

/**
 * Compact per-retriever outcome persisted on the episode (the full results
 * carry every retriever's article text — far too heavy to store).
 */
export type RetrieverAttempt =
  | { name: string; success: true; contentRating: number; textChars: number }
  | { name: string; success: false; error: string };

export function summarizeRetrieverAttempts(
  results: ArticleRetrieverResult[],
): RetrieverAttempt[] {
  return results.map((result) =>
    result.success
      ? {
          name: result.retrieverName,
          success: true,
          contentRating: result.metadata.info.contentRating,
          textChars: result.article.text.length,
        }
      : {
          name: result.retrieverName,
          success: false,
          error:
            result.error instanceof Error ? result.error.message : String(result.error),
        },
  );
}
