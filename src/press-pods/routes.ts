import { createHash, timingSafeEqual } from "node:crypto";
import { createReadStream } from "node:fs";
import fsAsync from "node:fs/promises";
import { Readable } from "node:stream";
import type { Logger } from "@micthiesen/mitools/logging";
import type { Context, Hono } from "hono";
import type { z } from "zod";
import {
  TaskAlreadyRunningError,
  TaskNotFoundError,
  type TaskRegistry,
} from "../task-runs/registry.js";
import config from "../utils/config.js";
import {
  deleteEpisode,
  getAllEpisodes,
  getAllJobs,
  getEpisode,
  getJob,
  jobNormalizedUrl,
  type PressPodsEpisodeData,
  type PressPodsJobData,
  PressPodsJobEntity,
  requeueJobNow,
} from "./persistence.js";
import { buildPressPodsFeed, latestEpisodeId } from "./rss.js";
import {
  AUDIO_FILE_RE,
  checkpointWorkId,
  clearChunkCheckpoints,
  deleteEpisodeAudio,
  episodeAudioPath,
} from "./storage.js";
import { submitEpisodeSchema, submitEpisodeUrl } from "./submit.js";

const LOGO_PATH = "assets/press-pods/logo.jpeg";

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

  app.post("/pods/episodes", async (c) => {
    if (!isAuthorized(c)) return c.json({ error: "Unauthorized" }, 401);
    let parsed: z.infer<typeof submitEpisodeSchema>;
    try {
      parsed = submitEpisodeSchema.parse(await c.req.json());
    } catch {
      return c.json({ error: "Body must be JSON: { url: string }" }, 400);
    }
    const job = submitEpisodeUrl(parsed.url, kickWorker, logger);
    return c.json({ jobId: job.jobId }, 202);
  });

  app.on(["GET", "HEAD"], "/pods/rss", (c) => {
    if (!isAuthorized(c)) return c.json({ error: "Unauthorized" }, 401);

    const etag = `"${latestEpisodeId()}"`;
    c.header("ETag", etag);
    c.header("Cache-Control", "no-cache");
    c.header("Content-Type", "application/xml; charset=utf-8");
    if (c.req.header("if-none-match") === etag) return c.body(null, 304);
    if (c.req.method === "HEAD") return c.body(null);
    return c.body(buildPressPodsFeed(resolveBaseUrl(c)));
  });

  app.on(["GET", "HEAD"], "/pods/audio/:file", async (c) => {
    const file = c.req.param("file");
    if (!AUDIO_FILE_RE.test(file)) return c.notFound();
    const filePath = episodeAudioPath(file);
    let size: number;
    try {
      size = (await fsAsync.stat(filePath)).size;
    } catch {
      return c.notFound();
    }

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
  });

  app.get("/pods/logo.jpeg", async (c) => {
    try {
      const logo = await fsAsync.readFile(LOGO_PATH);
      c.header("Content-Type", "image/jpeg");
      c.header("Cache-Control", "public, max-age=31536000, immutable");
      return c.body(new Uint8Array(logo).buffer as ArrayBuffer);
    } catch {
      return c.notFound();
    }
  });

  // -------------------------------------------------------------------------
  // Internal API for the web UI (same-origin; no token)
  // -------------------------------------------------------------------------

  app.get("/api/press-pods/episodes", (c) =>
    c.json({
      episodes: getAllEpisodes().map(serializeEpisode),
      jobs: getAllJobs().map(serializeJob),
    }),
  );

  app.get("/api/press-pods/episodes/:id", (c) => {
    const episode = getEpisode(c.req.param("id"));
    if (!episode) return c.json({ error: "Unknown episode" }, 404);
    return c.json({ episode: serializeEpisodeDetail(episode) });
  });

  // Manual delete from the UI: drop the row and its audio file. Episodes are
  // never pruned automatically, so this is the only way one goes away.
  app.delete("/api/press-pods/episodes/:id", async (c) => {
    const deleted = deleteEpisode(c.req.param("id"));
    if (!deleted) return c.json({ error: "Unknown episode" }, 404);
    await deleteEpisodeAudio(deleted.audioFile);
    logger.info(`Deleted episode ${deleted.episodeId} ("${deleted.title}")`);
    return c.json({ deleted: true });
  });

  // Manual retry/regenerate from the UI: re-run the article through the pipeline.
  // Goes through the shared submit path, so it dedups onto any in-flight job and
  // the fresh episode replaces this one on completion.
  app.post("/api/press-pods/episodes/:id/retry", (c) => {
    const episode = getEpisode(c.req.param("id"));
    if (!episode) return c.json({ error: "Unknown episode" }, 404);
    const job = submitEpisodeUrl(episode.articleUrl, kickWorker, logger);
    return c.json({ job: serializeJob(job) }, 202);
  });

  app.post("/api/press-pods/submit", async (c) => {
    let parsed: z.infer<typeof submitEpisodeSchema>;
    try {
      parsed = submitEpisodeSchema.parse(await c.req.json());
    } catch {
      return c.json({ error: "A valid article URL is required" }, 400);
    }
    const job = submitEpisodeUrl(parsed.url, kickWorker, logger);
    return c.json({ job: serializeJob(job) }, 202);
  });

  app.post("/api/press-pods/jobs/:jobId/retry", (c) => {
    const jobId = c.req.param("jobId");
    const existing = getJob(jobId);
    if (!existing) return c.json({ error: "Unknown job" }, 404);
    if (existing.status !== "failed") {
      return c.json({ error: "Only failed jobs can be retried" }, 409);
    }
    const job = requeueJobNow(jobId);
    if (!job) return c.json({ error: "Unknown job" }, 404);
    kickWorker();
    return c.json({ job: serializeJob(job) });
  });

  app.delete("/api/press-pods/jobs/:jobId", async (c) => {
    const jobId = c.req.param("jobId");
    const existing = getJob(jobId);
    if (!existing) return c.json({ error: "Unknown job" }, 404);
    if (existing.status === "processing") {
      return c.json({ error: "Job is currently processing" }, 409);
    }
    PressPodsJobEntity.delete({ jobId });
    // Dismissing a job means giving up on it — drop any per-chunk resume cache
    // so an abandoned article doesn't leave checkpoint WAVs on disk forever.
    await clearChunkCheckpoints(checkpointWorkId(jobNormalizedUrl(existing)));
    return c.json({ deleted: true });
  });
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
