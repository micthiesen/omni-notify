import type { Logger } from "@micthiesen/mitools/logging";
import { notify } from "@micthiesen/mitools/pushover";
import { formatDuration, getTitleFromUrl } from "@micthiesen/mitools/strings";
import { Clock, Effect } from "effect";
import config from "../utils/config.js";
import { getCleanedArticle } from "./agents/cleaner.js";
import { getDuration, tagEpisodeAudio } from "./audio.js";
import CostCounter from "./costs.js";
import { buildFinalText } from "./formatting/index.js";
import {
  type PressPodsEpisodeData,
  PressPodsPersistence,
  secureId,
} from "./persistence.js";
import { getArticleFromUrl } from "./retrievers/index.js";
import { synthesizeSpeech } from "./speech/synthesize.js";
import {
  checkpointWorkId,
  clearChunkCheckpoints,
  deleteEpisodeAudio,
  saveEpisodeAudio,
} from "./storage.js";
import { type Article, summarizeRetrieverAttempts } from "./types.js";
import { normalizeUrl } from "./url.js";
import { PressPodsError, tryPromise, trySync } from "./effect.js";

/**
 * URL → article retrieval → narration cleaning → TTS → audio finalization →
 * episode row → Pushover. Throws on failure; the caller (the PressPods task)
 * classifies the error and requeues or fails the job.
 */
export const createEpisodeFromUrl = Effect.fn("PressPods.createEpisode")(function* (
  url: string,
  runId: string | undefined,
  logger: Logger,
) {
  const start = yield* Clock.currentTimeMillis;
  const costCounter = new CostCounter();
  const normalizedUrl = yield* trySync("normalize PressPods article URL", () =>
    normalizeUrl(url),
  );
  const workId = yield* trySync("create PressPods checkpoint id", () =>
    checkpointWorkId(normalizedUrl),
  );

  const {
    article: unvalidatedArticle,
    metadata,
    retrieverName,
    allResults,
  } = yield* getArticleFromUrl(url, costCounter, logger);
  logger.info("Article retrieved", {
    title: unvalidatedArticle.title,
    chars: unvalidatedArticle.text.length,
    retriever: retrieverName,
  });

  const articleNoText = {
    title: metadata.info.title ?? unvalidatedArticle.title ?? getTitleFromUrl(url),
    author: metadata.info.author ?? unvalidatedArticle.author,
    domain: unvalidatedArticle.domain,
    url,
    publishedAt: metadata.info.publishedAtISO ?? unvalidatedArticle.publishedAt,
    leadImageUrl: metadata.info.leadImageUrl ?? unvalidatedArticle.leadImageUrl,
  } satisfies Omit<Article, "text">;
  if (articleNoText.author?.toLowerCase().includes("unknown")) {
    articleNoText.author = undefined;
  }
  const text = yield* trySync("format PressPods article", () =>
    buildFinalText({
      title: articleNoText.title,
      domain: metadata.info.publication ?? articleNoText.domain,
      author: articleNoText.author ?? "Anonymous",
      coauthors: metadata.info.coauthors,
      datePublished: articleNoText.publishedAt,
      text: unvalidatedArticle.text,
    }),
  );
  const article = { ...articleNoText, text } satisfies Article;

  const { content } = yield* getCleanedArticle(article, costCounter);
  logger.info("Narration text ready", { contentLength: content.length });

  const retrieverSeconds = ((yield* Clock.currentTimeMillis) - start) / 1000;

  const synthesis = yield* synthesizeSpeech({
    content,
    authorGender: metadata.info.authorGender,
    logger,
    costCounter,
    workId,
  });

  // Duration must be known before tagging so chapter end-times are correct;
  // ID3 tagging doesn't change duration, so measure the untagged audio first.
  const durationSeconds = yield* getDuration(synthesis.audio, logger);
  const audio = yield* tagEpisodeAudio(
    synthesis.audio,
    {
      leadImageUrl: article.leadImageUrl,
      chapters: synthesis.chapters,
      durationSeconds,
    },
    logger,
  );

  const episodeId = yield* trySync("create PressPods episode id", secureId);
  const audioFile = `${episodeId}.mp3`;
  const episode: PressPodsEpisodeData = {
    episodeId,
    title: article.title ?? getTitleFromUrl(url),
    author: article.author,
    authorGender: metadata.info.authorGender ?? undefined,
    publication: metadata.info.publication ?? undefined,
    domain: article.domain,
    articleUrl: url,
    normalizedUrl,
    leadImageUrl: article.leadImageUrl,
    excerpt: metadata.info.shortSummary ?? undefined,
    content,
    voiceName: synthesis.voiceName,
    voiceProvider: synthesis.voiceProvider,
    synthesizedSeconds: synthesis.synthesizedSeconds,
    chapters: synthesis.chapters,
    chunks: synthesis.chunks,
    audioFile,
    durationSeconds,
    fileBytes: audio.length,
    retrieverName,
    retrieverSeconds,
    retrieverAttempts: summarizeRetrieverAttempts(allResults),
    costs: costCounter.getCosts(),
    createdAt: yield* Clock.currentTimeMillis,
    publishedAt: article.publishedAt?.getTime(),
    runId,
  };
  yield* persistEpisodeWithAudio(episode, audio);

  // Resubmit-as-retry: the newest take replaces any older episode for the same
  // canonical URL. Do this right after the new row lands so a crash here can't
  // leave the article with zero episodes.
  yield* replaceOlderEpisodes(normalizedUrl, episodeId, logger);

  // Synthesis finished — the per-chunk resume cache for this article is no
  // longer needed.
  yield* clearChunkCheckpoints(workId);

  logger.info(`Episode created for "${episode.title}"`, costCounter.getCosts());
  yield* notifyEpisodeAvailable(episode, logger);
  return episode;
});

/**
 * Commit the file then its referencing row. If the durable row cannot land,
 * remove the just-written final MP3 so a persistence outage cannot accumulate
 * invisible orphan audio.
 */
export const persistEpisodeWithAudio = Effect.fn("PressPods.persistEpisode")(function* (
  episode: PressPodsEpisodeData,
  audio: Buffer,
  persist: (
    episode: PressPodsEpisodeData,
  ) => Effect.Effect<void, PressPodsError> = PressPodsPersistence.upsertEpisode,
) {
  yield* saveEpisodeAudio(episode.audioFile, audio);
  yield* persist(episode).pipe(
    Effect.onError(() => deleteEpisodeAudio(episode.audioFile)),
  );
});

/**
 * Drop episodes older than `keepEpisodeId` that share its canonical URL, plus
 * their audio files. Runs on the happy path (createEpisodeFromUrl) and on
 * crash recovery (the worker completing a job whose episode already landed) so
 * the replace invariant holds even if the process died in the gap between the
 * new episode's write and this cleanup.
 */
export const replaceOlderEpisodes = Effect.fn("PressPods.replaceOlderEpisodes")(
  function* (normalizedUrl: string, keepEpisodeId: string, logger: Logger) {
    const replaced = yield* PressPodsPersistence.deleteEpisodesByNormalizedUrlExcept(
      normalizedUrl,
      keepEpisodeId,
    );
    yield* Effect.forEach(
      replaced,
      (old) =>
        Effect.gen(function* () {
          yield* deleteEpisodeAudio(old.audioFile);
          logger.info(`Replaced older episode ${old.episodeId} for the same article`);
        }),
      { discard: true },
    );
  },
);

function notifyEpisodeAvailable(
  episode: PressPodsEpisodeData,
  logger: Logger,
): Effect.Effect<void> {
  const costs = episode.costs;
  const totalCents = (costs?.llmCents ?? 0) + (costs?.ttsCents ?? 0);
  const parts = [
    `'${episode.title}' from '${episode.domain ?? "unknown"}' is now available.`,
    `${formatDuration(episode.durationSeconds)} · ${episode.voiceName} · US$${(totalCents / 100).toFixed(2)}`,
  ];
  return tryPromise("notify for new PressPods episode", () =>
    notify({
      title: "Episode Now Available",
      message: parts.join("\n"),
      token: config.PUSHOVER_PRESSPODS_TOKEN,
      url: `${config.RECS_PUBLIC_URL}/pods`,
      url_title: "Open PressPods",
    }),
  ).pipe(
    Effect.catch((error) => {
      // The episode exists and the feed will pick it up; delivery is best-effort.
      logger.warn("Failed to send episode notification", { error });
      return Effect.void;
    }),
  );
}
