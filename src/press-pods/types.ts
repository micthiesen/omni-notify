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

/** A chapter marker: title + its start offset (seconds) into the final audio. */
export interface Chapter {
  startTimeSeconds: number;
  title: string;
}

/**
 * Per-chunk synthesis stats, persisted on the episode for diagnostics (the
 * detail page can chart pacing, denoise/retry behavior, etc). `attempts` is
 * how many synth takes the length-verify retry loop spent (1 for providers
 * that skip verification, e.g. ElevenLabs).
 */
export interface ChunkStat {
  index: number;
  sectionIndex: number;
  sectionTitle?: string;
  text: string;
  charCount: number;
  durationSeconds: number;
  /** Offset into the final audio (includes the intro jingle), like Chapter. */
  startTimeSeconds: number;
  secPerChar: number;
  attempts: number;
  /**
   * STT content-verification result for the chosen take (Higgs only; absent
   * when the provider skips content verification or no STT endpoint is set).
   * `coverage` is the fraction of input words recovered from the audio, ~1 for
   * a complete read; `wordRatio` is transcript/input word count.
   */
  coverage?: number;
  wordRatio?: number;
  /**
   * True when this piece came from adaptively re-splitting a larger chunk that
   * kept failing verification (see synthesize.ts). Recovery worked, but it marks
   * where Higgs struggled — a chunk that needed re-splitting is worth a look even
   * if every sub-piece ultimately passed.
   */
  resplit?: boolean;
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
