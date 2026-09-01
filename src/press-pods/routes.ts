import { createHash, timingSafeEqual } from "node:crypto";
import { createReadStream } from "node:fs";
import fsAsync from "node:fs/promises";
import { Readable } from "node:stream";
import type { Logger } from "@micthiesen/mitools/logging";
import type { Context, Hono } from "hono";
import { Effect, Schema } from "effect";
import { decodeJsonBody, effectHandler } from "../effect/http.js";
import { fromPromise } from "../effect/interop.js";
import {
  TaskAlreadyRunningError,
  TaskNotFoundError,
  type TaskRegistry,
} from "../task-runs/registry.js";
import config from "../utils/config.js";
import {
  jobNormalizedUrl,
  type PressPodsEpisodeData,
  type PressPodsJobData,
  PressPodsJobEntity,
  PressPodsPersistence,
} from "./persistence.js";
import { PressPodsError, trySync } from "./effect.js";
import { assertPublicHttpUrlSyntax } from "./publicHttp.js";
import { buildPressPodsFeedEffect, latestEpisodeIdEffect } from "./rss.js";
import {
  AUDIO_FILE_RE,
  checkpointWorkId,
  clearChunkCheckpoints,
  deleteEpisodeAudio,
  episodeAudioPath,
} from "./storage.js";
import { submitEpisodeUrlEffect } from "./submit.js";

const LOGO_PATH = "assets/press-pods/logo.jpeg";
const SubmitEpisodeBodySchema = Schema.Struct({ url: Schema.String });

function decodeSubmitUrlEffect(c: Context) {
  return decodeJsonBody(c, SubmitEpisodeBodySchema).pipe(
    Effect.map((body) => body.url.split("\n")[0].trim()),
    Effect.flatMap((url) =>
      trySync("validate PressPods article URL", () => {
        assertPublicHttpUrlSyntax(url);
        return url;
      }),
    ),
  );
}

/**
 * PressPods HTTP surface. The `/pods/*` routes are meant to be exposed
 * publicly through a reverse proxy for the iOS Shortcut and the podcast
 * client: submissions and the feed require the auth token, audio files rely
 * on unguessable content-addressed names (podcast apps can't send headers on
 * enclosure fetches). The `/api/press-pods/*` routes serve the web UI.
 */
export function registerPressPodsRoutes(
  app: Hono,
  registry: TaskRegistry,
  parentLogger: Logger,
): void {
  if (!config.PRESSPODS_AUTH_TOKEN) return;
  const logger = parentLogger.extend("PressPods");

  // The routes gate only on the auth token, but the worker task also needs
  // TTS/model credentials — without them submissions would queue forever with
  // no error anywhere. Make that misconfiguration loud at boot.
  const ttsCredMissing =
    config.PRESSPODS_TTS_PROVIDER === "elevenlabs"
      ? !config.ELEVENLABS_API_KEY && "ELEVENLABS_API_KEY"
      : !config.PRESSPODS_TTS_URL && "PRESSPODS_TTS_URL";
  if (ttsCredMissing) {
    logger.warn(
      `PressPods routes are active but the worker task is disabled ` +
        `(missing ${ttsCredMissing}); submitted jobs will queue without processing`,
    );
  }

  const kickWorker = (): void => {
    try {
      registry.runNow("PressPods");
    } catch (error) {
      // Already running (the drain loop will pick the job up) or server-only
      // mode (no tasks registered; the job waits for a worker process).
      if (
        !(error instanceof TaskAlreadyRunningError) &&
        !(error instanceof TaskNotFoundError)
      ) {
        throw error;
      }
    }
  };

  // -------------------------------------------------------------------------
  // Public routes (token-gated; expose /pods/* through the reverse proxy)
  // -------------------------------------------------------------------------

  app.post(
    "/pods/episodes",
    effectHandler((c) => {
      if (!isAuthorized(c)) {
        return Effect.succeed(c.json({ error: "Unauthorized" }, 401));
      }
      return Effect.result(decodeSubmitUrlEffect(c)).pipe(
        Effect.flatMap((decoded): Effect.Effect<Response, PressPodsError> =>
          decoded._tag === "Failure"
            ? Effect.succeed<Response>(
                c.json({ error: "Body must be JSON: { url: string }" }, 400),
              )
            : submitEpisodeUrlEffect(decoded.success, kickWorker, logger).pipe(
                Effect.map((job): Response => c.json({ jobId: job.jobId }, 202)),
              ),
        ),
      );
    }),
  );

  app.on(
    ["GET", "HEAD"],
    "/pods/rss",
    effectHandler((c) => {
      if (!isAuthorized(c)) {
        return Effect.succeed(c.json({ error: "Unauthorized" }, 401));
      }
      return latestEpisodeIdEffect().pipe(
        Effect.flatMap((latestId): Effect.Effect<Response, PressPodsError> => {
          const etag = `"${latestId}"`;
          c.header("ETag", etag);
          c.header("Cache-Control", "no-cache");
          c.header("Content-Type", "application/xml; charset=utf-8");
          if (c.req.header("if-none-match") === etag) {
            return Effect.succeed<Response>(c.body(null, 304));
          }
          if (c.req.method === "HEAD") {
            return Effect.succeed<Response>(c.body(null));
          }
          return buildPressPodsFeedEffect(resolveBaseUrl(c)).pipe(
            Effect.map((feed): Response => c.body(feed)),
          );
        }),
      );
    }),
  );

  app.on(
    ["GET", "HEAD"],
    "/pods/audio/:file",
    effectHandler((c) => {
      const file = c.req.param("file");
      if (!file || !AUDIO_FILE_RE.test(file)) {
        return Effect.succeed(c.body(null, 404));
      }
      const filePath = episodeAudioPath(file);
      return Effect.result(
        fromPromise("inspect PressPods audio file", () => fsAsync.stat(filePath)),
      ).pipe(
        Effect.map((result) => {
          if (result._tag === "Failure") return c.body(null, 404);
          const size = result.success.size;
          c.header("Accept-Ranges", "bytes");
          c.header("Content-Type", "audio/mpeg");
          // Content-addressed name: the file never changes once written.
          c.header("Cache-Control", "public, max-age=31536000, immutable");

          const range = parseByteRange(c.req.header("range"), size);
          if (range === "invalid") {
            c.header("Content-Range", `bytes */${size}`);
            return c.body(null, 416);
          }
          if (c.req.method === "HEAD") {
            c.header("Content-Length", String(size));
            return c.body(null);
          }
          if (range) {
            c.header("Content-Range", `bytes ${range.start}-${range.end}/${size}`);
            c.header("Content-Length", String(range.end - range.start + 1));
            const stream = createReadStream(filePath, {
              start: range.start,
              end: range.end,
            });
            return c.body(Readable.toWeb(stream) as ReadableStream, 206);
          }
          c.header("Content-Length", String(size));
          return c.body(Readable.toWeb(createReadStream(filePath)) as ReadableStream);
        }),
      );
    }),
  );

  app.get(
    "/pods/logo.jpeg",
    effectHandler((c) =>
      Effect.result(
        fromPromise("read PressPods logo", () => fsAsync.readFile(LOGO_PATH)),
      ).pipe(
        Effect.map((result) => {
          if (result._tag === "Failure") return c.body(null, 404);
          c.header("Content-Type", "image/jpeg");
          c.header("Cache-Control", "public, max-age=31536000, immutable");
          return c.body(new Uint8Array(result.success).buffer as ArrayBuffer);
        }),
      ),
    ),
  );

  // -------------------------------------------------------------------------
  // Internal API for the web UI (same-origin; no token)
  // -------------------------------------------------------------------------

  app.get(
    "/api/press-pods/episodes",
    effectHandler((c) =>
      Effect.all(
        [PressPodsPersistence.getAllEpisodes(), PressPodsPersistence.getAllJobs()],
        { concurrency: 2 },
      ).pipe(
        Effect.map(([episodes, jobs]) =>
          c.json({
            episodes: episodes.map(serializeEpisode),
            jobs: jobs.map(serializeJob),
          }),
        ),
      ),
    ),
  );

  app.get(
    "/api/press-pods/episodes/:id",
    effectHandler((c) =>
      PressPodsPersistence.getEpisode(c.req.param("id") ?? "").pipe(
        Effect.map((episode) =>
          episode
            ? c.json({ episode: serializeEpisodeDetail(episode) })
            : c.json({ error: "Unknown episode" }, 404),
        ),
      ),
    ),
  );

  // Manual delete from the UI: drop the row and its audio file. Episodes are
  // never pruned automatically, so this is the only way one goes away.
  app.delete(
    "/api/press-pods/episodes/:id",
    effectHandler((c) =>
      PressPodsPersistence.deleteEpisode(c.req.param("id") ?? "").pipe(
        Effect.flatMap((deleted): Effect.Effect<Response> => {
          if (!deleted) {
            return Effect.succeed<Response>(c.json({ error: "Unknown episode" }, 404));
          }
          return deleteEpisodeAudio(deleted.audioFile).pipe(
            Effect.tap(() =>
              Effect.sync(() =>
                logger.info(
                  `Deleted episode ${deleted.episodeId} ("${deleted.title}")`,
                ),
              ),
            ),
            Effect.map((): Response => c.json({ deleted: true })),
          );
        }),
      ),
    ),
  );

  // Manual retry/regenerate from the UI: re-run the article through the pipeline.
  // Goes through the shared submit path, so it dedups onto any in-flight job and
  // the fresh episode replaces this one on completion.
  app.post(
    "/api/press-pods/episodes/:id/retry",
    effectHandler((c) =>
      PressPodsPersistence.getEpisode(c.req.param("id") ?? "").pipe(
        Effect.flatMap((episode): Effect.Effect<Response, PressPodsError> =>
          episode
            ? submitEpisodeUrlEffect(episode.articleUrl, kickWorker, logger).pipe(
                Effect.map((job): Response => c.json({ job: serializeJob(job) }, 202)),
              )
            : Effect.succeed<Response>(c.json({ error: "Unknown episode" }, 404)),
        ),
      ),
    ),
  );

  app.post(
    "/api/press-pods/submit",
    effectHandler((c) =>
      Effect.result(decodeSubmitUrlEffect(c)).pipe(
        Effect.flatMap((decoded): Effect.Effect<Response, PressPodsError> =>
          decoded._tag === "Failure"
            ? Effect.succeed<Response>(
                c.json({ error: "A valid article URL is required" }, 400),
              )
            : submitEpisodeUrlEffect(decoded.success, kickWorker, logger).pipe(
                Effect.map((job): Response => c.json({ job: serializeJob(job) }, 202)),
              ),
        ),
      ),
    ),
  );

  app.post(
    "/api/press-pods/jobs/:jobId/retry",
    effectHandler((c) => {
      const jobId = c.req.param("jobId") ?? "";
      return PressPodsPersistence.getJob(jobId).pipe(
        Effect.flatMap((existing): Effect.Effect<Response, PressPodsError> => {
          if (!existing) {
            return Effect.succeed<Response>(c.json({ error: "Unknown job" }, 404));
          }
          if (existing.status !== "failed") {
            return Effect.succeed<Response>(
              c.json({ error: "Only failed jobs can be retried" }, 409),
            );
          }
          return PressPodsPersistence.requeueJobNow(jobId).pipe(
            Effect.flatMap((job): Effect.Effect<Response, PressPodsError> => {
              if (!job) {
                return Effect.succeed<Response>(c.json({ error: "Unknown job" }, 404));
              }
              return trySync("kick PressPods worker", kickWorker).pipe(
                Effect.map((): Response => c.json({ job: serializeJob(job) })),
              );
            }),
          );
        }),
      );
    }),
  );

  app.delete(
    "/api/press-pods/jobs/:jobId",
    effectHandler((c) => {
      const jobId = c.req.param("jobId") ?? "";
      return PressPodsPersistence.getJob(jobId).pipe(
        Effect.flatMap((existing): Effect.Effect<Response, PressPodsError> => {
          if (!existing) {
            return Effect.succeed<Response>(c.json({ error: "Unknown job" }, 404));
          }
          if (existing.status === "processing") {
            return Effect.succeed<Response>(
              c.json({ error: "Job is currently processing" }, 409),
            );
          }
          return trySync("delete PressPods job", () =>
            PressPodsJobEntity.delete({ jobId }),
          ).pipe(
            // Dismissing a job means giving up on it, including resume cache.
            Effect.andThen(
              clearChunkCheckpoints(checkpointWorkId(jobNormalizedUrl(existing))),
            ),
            Effect.map((): Response => c.json({ deleted: true })),
          );
        }),
      );
    }),
  );
}

function isAuthorized(c: Context): boolean {
  const provided = c.req.query("authToken") ?? c.req.header("x-auth-token");
  const expected = config.PRESSPODS_AUTH_TOKEN;
  if (!provided || !expected) return false;
  // Hash both sides so timingSafeEqual gets equal-length buffers.
  const providedDigest = createHash("sha256").update(provided).digest();
  const expectedDigest = createHash("sha256").update(expected).digest();
  return timingSafeEqual(providedDigest, expectedDigest);
}

/** Public origin for enclosure URLs: config wins, else forwarded headers. */
function resolveBaseUrl(c: Context): string {
  if (config.PRESSPODS_PUBLIC_URL) return config.PRESSPODS_PUBLIC_URL;
  const host = c.req.header("x-forwarded-host") ?? c.req.header("host") ?? "localhost";
  const proto = c.req.header("x-forwarded-proto") ?? "http";
  return `${proto}://${host}`;
}

export function parseByteRange(
  header: string | undefined,
  size: number,
): { start: number; end: number } | "invalid" | undefined {
  if (!header) return undefined;
  const match = header.match(/^bytes=(\d*)-(\d*)$/);
  if (!match) return "invalid";
  const [, startStr, endStr] = match;
  if (startStr === "" && endStr === "") return "invalid";
  if (startStr === "") {
    // Suffix range: last N bytes
    const suffix = Number(endStr);
    if (suffix === 0) return "invalid";
    return { start: Math.max(0, size - suffix), end: size - 1 };
  }
  const start = Number(startStr);
  const end = endStr === "" ? size - 1 : Math.min(Number(endStr), size - 1);
  if (start >= size || start > end) return "invalid";
  return { start, end };
}

function serializeEpisode(episode: PressPodsEpisodeData) {
  return {
    episodeId: episode.episodeId,
    title: episode.title,
    author: episode.author ?? null,
    publication: episode.publication ?? null,
    domain: episode.domain ?? null,
    articleUrl: episode.articleUrl,
    leadImageUrl: episode.leadImageUrl ?? null,
    excerpt: episode.excerpt ?? null,
    voiceName: episode.voiceName ?? null,
    synthesizedSeconds: episode.synthesizedSeconds ?? null,
    chapters: episode.chapters ?? null,
    audioUrl: `/pods/audio/${episode.audioFile}`,
    durationSeconds: episode.durationSeconds ?? null,
    fileBytes: episode.fileBytes,
    retrieverName: episode.retrieverName ?? null,
    retrieverSeconds: episode.retrieverSeconds ?? null,
    retrieverAttempts: episode.retrieverAttempts ?? null,
    costCents: episode.costs
      ? Math.round((episode.costs.llmCents + episode.costs.ttsCents) * 100) / 100
      : null,
    createdAt: episode.createdAt,
    publishedAt: episode.publishedAt ?? null,
    runId: episode.runId ?? null,
  };
}

/**
 * Full episode detail for the `/pods/:id` page: everything the list
 * serializer sends plus the transcript, per-chunk synthesis stats, and the
 * itemized cost breakdown. Deliberately not part of the list payload — those
 * fields are too heavy to ship for every row.
 */
function serializeEpisodeDetail(episode: PressPodsEpisodeData) {
  return {
    ...serializeEpisode(episode),
    content: episode.content,
    authorGender: episode.authorGender ?? null,
    voiceProvider: episode.voiceProvider ?? null,
    chunks: episode.chunks ?? null,
    costs: episode.costs ?? null,
  };
}

function serializeJob(job: PressPodsJobData) {
  return {
    jobId: job.jobId,
    url: job.url,
    status: job.status,
    attempts: job.attempts,
    nextAttemptAt: job.nextAttemptAt || null,
    lastError: job.lastError ?? null,
    createdAt: job.createdAt,
    updatedAt: job.updatedAt,
    lastRunId: job.lastRunId ?? null,
  };
}
