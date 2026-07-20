import type { Logger } from "@micthiesen/mitools/logging";
import { notify } from "@micthiesen/mitools/pushover";
import { formatDuration, getTitleFromUrl } from "@micthiesen/mitools/strings";
import config from "../utils/config.js";
import { getCleanedArticle } from "./agents/cleaner.js";
import { getDuration, tagEpisodeAudio } from "./audio.js";
import CostCounter from "./costs.js";
import { buildFinalText } from "./formatting/index.js";
import {
  deleteEpisodesByNormalizedUrlExcept,
  type PressPodsEpisodeData,
  PressPodsEpisodeEntity,
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

/**
 * URL → article retrieval → narration cleaning → TTS → audio finalization →
 * episode row → Pushover. Throws on failure; the caller (the PressPods task)
 * classifies the error and requeues or fails the job.
 */
export async function createEpisodeFromUrl(
  url: string,
  runId: string | undefined,
  logger: Logger,
): Promise<PressPodsEpisodeData> {
  const start = Date.now();
  const costCounter = new CostCounter();
  const normalizedUrl = normalizeUrl(url);
  const workId = checkpointWorkId(normalizedUrl);

  const {
    article: unvalidatedArticle,
    metadata,
    retrieverName,
    allResults,
  } = await getArticleFromUrl(url, costCounter, logger);
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
  const text = buildFinalText({
    title: articleNoText.title,
    domain: metadata.info.publication ?? articleNoText.domain,
    author: articleNoText.author ?? "Anonymous",
    coauthors: metadata.info.coauthors,
    datePublished: articleNoText.publishedAt,
    text: unvalidatedArticle.text,
  });
  const article = { ...articleNoText, text } satisfies Article;

  const { content } = await getCleanedArticle(article, costCounter);
  logger.info("Narration text ready", { contentLength: content.length });

  const retrieverSeconds = (Date.now() - start) / 1000;

  const synthesis = await synthesizeSpeech({
    content,
    authorGender: metadata.info.authorGender,
    logger,
    costCounter,
    workId,
  });

  // Duration must be known before tagging so chapter end-times are correct;
  // ID3 tagging doesn't change duration, so measure the untagged audio first.
  const durationSeconds = await getDuration(synthesis.audio, logger);
  const audio = await tagEpisodeAudio(
    synthesis.audio,
    {
      leadImageUrl: article.leadImageUrl,
      chapters: synthesis.chapters,
      durationSeconds,
    },
    logger,
  );

  const episodeId = secureId();
  const audioFile = `${episodeId}.mp3`;
  await saveEpisodeAudio(audioFile, audio);

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
    createdAt: Date.now(),
    publishedAt: article.publishedAt?.getTime(),
    runId,
  };
  PressPodsEpisodeEntity.upsert(episode);

  // Resubmit-as-retry: the newest take replaces any older episode for the same
  // canonical URL. Do this right after the new row lands so a crash here can't
  // leave the article with zero episodes.
  await replaceOlderEpisodes(normalizedUrl, episodeId, logger);

  // Synthesis finished — the per-chunk resume cache for this article is no
  // longer needed.
  await clearChunkCheckpoints(workId);

  logger.info(`Episode created for "${episode.title}"`, costCounter.getCosts());
  await notifyEpisodeAvailable(episode, logger);
  return episode;
}

/**
 * Drop episodes older than `keepEpisodeId` that share its canonical URL, plus
 * their audio files. Runs on the happy path (createEpisodeFromUrl) and on
 * crash recovery (the worker completing a job whose episode already landed) so
 * the replace invariant holds even if the process died in the gap between the
 * new episode's write and this cleanup.
 */
export async function replaceOlderEpisodes(
  normalizedUrl: string,
  keepEpisodeId: string,
  logger: Logger,
): Promise<void> {
  const replaced = deleteEpisodesByNormalizedUrlExcept(normalizedUrl, keepEpisodeId);
  for (const old of replaced) {
    await deleteEpisodeAudio(old.audioFile);
    logger.info(`Replaced older episode ${old.episodeId} for the same article`);
  }
}

async function notifyEpisodeAvailable(
  episode: PressPodsEpisodeData,
  logger: Logger,
): Promise<void> {
  const costs = episode.costs;
  const totalCents = (costs?.llmCents ?? 0) + (costs?.ttsCents ?? 0);
  const parts = [
    `'${episode.title}' from '${episode.domain ?? "unknown"}' is now available.`,
    `${formatDuration(episode.durationSeconds)} · ${episode.voiceName} · US$${(totalCents / 100).toFixed(2)}`,
  ];
  try {
    await notify({
      title: "Episode Now Available",
      message: parts.join("\n"),
      token: config.PUSHOVER_PRESSPODS_TOKEN,
      url: `${config.RECS_PUBLIC_URL}/pods`,
      url_title: "Open PressPods",
    });
  } catch (error) {
    // The episode exists and the feed will pick it up; delivery is best-effort.
    logger.warn("Failed to send episode notification", { error });
  }
}
