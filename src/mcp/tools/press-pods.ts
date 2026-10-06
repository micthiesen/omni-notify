import { Effect } from "effect";
import { z } from "zod";
import {
  jobNormalizedUrl,
  PressPodsPersistence,
  type PressPodsEpisodeData,
  type PressPodsJobData,
} from "../../press-pods/persistence.js";
import { assertPublicHttpUrl } from "../../press-pods/publicHttp.js";
import {
  checkpointWorkId,
  clearChunkCheckpoints,
  deleteEpisodeAudio,
} from "../../press-pods/storage.js";
import {
  submitEpisodeSchema,
  submitEpisodeUrlEffect,
} from "../../press-pods/submit.js";
import {
  TaskAlreadyRunningError,
  TaskNotFoundError,
} from "../../task-runs/registry.js";
import type { McpRuntime } from "../runtime.js";
import {
  annotations,
  defineTool,
  type McpToolDefinition,
  paginate,
  paginationInputShape,
  truncate,
} from "../tool.js";
import { pageSchema } from "./media-shared.js";

const nullableString = z.string().nullable();

const nullableNumber = z.number().nullable();

const pressPodsEpisodeSchema = z.object({
  episodeId: z.string(),
  title: z.string(),
  author: nullableString,
  publication: nullableString,
  domain: nullableString,
  articleUrl: z.string().url(),
  excerpt: nullableString,
  voiceName: nullableString,
  voiceProvider: nullableString,
  durationSeconds: nullableNumber,
  fileBytes: z.number().int().nonnegative(),
  retrieverName: nullableString,
  costCents: nullableNumber,
  createdAt: z.number().int(),
  publishedAt: nullableNumber,
  runId: nullableString,
  chapterCount: z.number().int().nonnegative(),
});

const pressPodsJobSchema = z.object({
  jobId: z.string(),
  url: z.string().url(),
  status: z.enum(["queued", "processing", "failed"]),
  attempts: z.number().int().nonnegative(),
  nextAttemptAt: nullableNumber,
  lastError: nullableString,
  createdAt: z.number().int(),
  updatedAt: z.number().int(),
  lastRunId: nullableString,
});

function serializeEpisode(episode: PressPodsEpisodeData) {
  return {
    episodeId: episode.episodeId,
    title: episode.title,
    author: episode.author ?? null,
    publication: episode.publication ?? null,
    domain: episode.domain ?? null,
    articleUrl: episode.articleUrl,
    excerpt: episode.excerpt ?? null,
    voiceName: episode.voiceName ?? null,
    voiceProvider: episode.voiceProvider ?? null,
    durationSeconds: episode.durationSeconds ?? null,
    fileBytes: episode.fileBytes,
    retrieverName: episode.retrieverName ?? null,
    costCents: episode.costs
      ? Math.round((episode.costs.llmCents + episode.costs.ttsCents) * 100) / 100
      : null,
    createdAt: episode.createdAt,
    publishedAt: episode.publishedAt ?? null,
    runId: episode.runId ?? null,
    chapterCount: episode.chapters?.length ?? 0,
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

function kickPressPods(runtime: McpRuntime) {
  return runtime.registry.runNow("PressPods").pipe(
    Effect.catch((error) => {
      if (
        error instanceof TaskAlreadyRunningError ||
        error instanceof TaskNotFoundError
      ) {
        return Effect.void;
      }
      return Effect.fail(error);
    }),
    Effect.asVoid,
  );
}

export function createPressPodsTools(runtime: McpRuntime): McpToolDefinition[] {
  return [
    defineTool({
      name: "presspods_list",
      title: "List PressPods Resources",
      description:
        "List persisted PressPods episodes or queued, processing, and failed jobs. Audio filenames and bytes are not exposed.",
      inputSchema: z
        .object({
          resource: z.enum(["episodes", "jobs"]),
          status: z.enum(["queued", "processing", "failed"]).optional(),
          query: z.string().trim().max(300).optional(),
          cursor: paginationInputShape.cursor,
          limit: paginationInputShape.limit,
        })
        .strict(),
      outputSchema: z.discriminatedUnion("resource", [
        pageSchema.extend({
          resource: z.literal("episodes"),
          items: z.array(pressPodsEpisodeSchema),
        }),
        pageSchema.extend({
          resource: z.literal("jobs"),
          items: z.array(pressPodsJobSchema),
        }),
      ]),
      annotations: annotations(true, false, true, false),
      policy: {
        sideEffects: [],
        cost: "No external traffic or monetary cost",
        recommendedPolicy: "allow",
      },
      execute: ({ resource, status, query, cursor, limit }) =>
        Effect.gen(function* () {
          if (resource === "episodes") {
            let values = yield* PressPodsPersistence.getAllEpisodes();
            if (query) {
              const needle = query.toLocaleLowerCase();
              values = values.filter((item) =>
                [item.title, item.author, item.publication, item.domain].some((value) =>
                  value?.toLocaleLowerCase().includes(needle),
                ),
              );
            }
            return {
              resource,
              ...paginate(values.map(serializeEpisode), cursor, limit),
            };
          }
          let values = yield* PressPodsPersistence.getAllJobs();
          if (status) values = values.filter((item) => item.status === status);
          if (query) {
            const needle = query.toLocaleLowerCase();
            values = values.filter((item) =>
              item.url.toLocaleLowerCase().includes(needle),
            );
          }
          return { resource, ...paginate(values.map(serializeJob), cursor, limit) };
        }),
    }),
    defineTool({
      name: "presspods_episode_get",
      title: "Get PressPods Episode",
      description:
        "Get compact metadata for one PressPods episode. Use presspods_transcript_read for bounded narration text.",
      inputSchema: z.object({ episodeId: z.string().min(1).max(200) }).strict(),
      outputSchema: z.object({ episode: pressPodsEpisodeSchema }),
      annotations: annotations(true, false, true, false),
      policy: {
        sideEffects: [],
        cost: "No external traffic or monetary cost",
        recommendedPolicy: "allow",
      },
      execute: ({ episodeId }) =>
        Effect.gen(function* () {
          const episode = yield* PressPodsPersistence.getEpisode(episodeId);
          if (!episode)
            return yield* Effect.fail(new Error("PressPods episode not found"));
          return { episode: serializeEpisode(episode) };
        }),
    }),
    defineTool({
      name: "presspods_transcript_read",
      title: "Read PressPods Transcript",
      description:
        "Read a bounded page of an episode's cleaned narration transcript. This never returns audio bytes or filesystem paths.",
      inputSchema: z
        .object({
          episodeId: z.string().min(1).max(200),
          offset: z.number().int().min(0).default(0),
          maxChars: z.number().int().min(1).max(10_000).default(4000),
        })
        .strict(),
      outputSchema: z.object({
        episodeId: z.string(),
        title: z.string(),
        offset: z.number().int().nonnegative(),
        text: z.string(),
        nextOffset: z.number().int().nonnegative().nullable(),
        totalChars: z.number().int().nonnegative(),
        truncated: z.boolean(),
      }),
      annotations: annotations(true, false, true, false),
      policy: {
        sideEffects: [],
        cost: "No external traffic or monetary cost",
        recommendedPolicy: "allow",
      },
      execute: ({ episodeId, offset, maxChars }) =>
        Effect.gen(function* () {
          const episode = yield* PressPodsPersistence.getEpisode(episodeId);
          if (!episode)
            return yield* Effect.fail(new Error("PressPods episode not found"));
          if (offset > episode.content.length)
            return yield* Effect.fail(
              new Error("Transcript offset is beyond the end of the episode"),
            );
          const page = truncate(episode.content.slice(offset), maxChars);
          return {
            episodeId,
            title: episode.title,
            offset,
            text: page.text,
            nextOffset: page.truncated ? offset + page.text.length : null,
            totalChars: episode.content.length,
            truncated: page.truncated,
          };
        }),
    }),
    defineTool({
      name: "presspods_submit",
      title: "Submit PressPods Episode",
      description:
        "Queue a public article URL for retrieval, model cleaning, TTS synthesis, podcast publication, optional Karakeep bookmarking, and notification. This has material compute/model cost and external effects, so Executor approval is required.",
      inputSchema: submitEpisodeSchema.strict(),
      outputSchema: z.object({ job: pressPodsJobSchema }),
      annotations: annotations(false, false, false, true),
      policy: {
        sideEffects: [
          "Queues paid or self-hosted model and TTS work",
          "May bookmark the URL in Karakeep",
          "Publishes an episode to the personal feed and sends a notification after processing",
        ],
        cost: "May incur metadata/cleaning model and TTS charges; ElevenLabs is approximately $0.10 per 1,000 characters when configured",
        recommendedPolicy: "require_approval",
      },
      execute: ({ url }) =>
        Effect.gen(function* () {
          yield* assertPublicHttpUrl(url);
          const job = yield* submitEpisodeUrlEffect(
            url,
            () => kickPressPods(runtime),
            runtime.logger.extend("MCP:PressPods"),
          );
          return { job: serializeJob(job) };
        }),
    }),
    defineTool({
      name: "presspods_retry",
      title: "Retry PressPods Work",
      description:
        "Regenerate an existing episode or retry a failed job. This starts model/TTS work and can replace a published episode, so Executor approval is required.",
      inputSchema: z.discriminatedUnion("resource", [
        z.object({
          resource: z.literal("episode"),
          episodeId: z.string().min(1).max(200),
        }),
        z.object({ resource: z.literal("job"), jobId: z.string().min(1).max(200) }),
      ]),
      outputSchema: z.object({ job: pressPodsJobSchema }),
      annotations: annotations(false, false, false, true),
      policy: {
        sideEffects: [
          "Queues paid or self-hosted model and TTS work",
          "May replace an existing published episode and send a notification",
        ],
        cost: "May incur metadata/cleaning model and TTS charges",
        recommendedPolicy: "require_approval",
      },
      execute: (input) =>
        Effect.gen(function* () {
          let job: PressPodsJobData;
          if (input.resource === "episode") {
            const episode = yield* PressPodsPersistence.getEpisode(input.episodeId);
            if (!episode)
              return yield* Effect.fail(new Error("PressPods episode not found"));
            job = yield* submitEpisodeUrlEffect(
              episode.articleUrl,
              () => kickPressPods(runtime),
              runtime.logger.extend("MCP:PressPods"),
            );
          } else {
            const existing = yield* PressPodsPersistence.getJob(input.jobId);
            if (!existing)
              return yield* Effect.fail(new Error("PressPods job not found"));
            if (existing.status !== "failed")
              return yield* Effect.fail(
                new Error("Only failed PressPods jobs can be retried"),
              );
            const requeued = yield* PressPodsPersistence.requeueJobNow(input.jobId);
            if (!requeued)
              return yield* Effect.fail(
                new Error("PressPods job could not be retried"),
              );
            job = requeued;
            yield* kickPressPods(runtime);
          }
          return { job: serializeJob(job) };
        }),
    }),
    defineTool({
      name: "presspods_delete",
      title: "Delete PressPods Resource",
      description:
        "Permanently delete an episode and its audio, or dismiss a non-processing job and its resume checkpoints. This is destructive and requires Executor approval.",
      inputSchema: z.discriminatedUnion("resource", [
        z.object({
          resource: z.literal("episode"),
          episodeId: z.string().min(1).max(200),
        }),
        z.object({ resource: z.literal("job"), jobId: z.string().min(1).max(200) }),
      ]),
      outputSchema: z.object({
        resource: z.enum(["episode", "job"]),
        deleted: z.literal(true),
      }),
      annotations: annotations(false, true, true, false),
      policy: {
        sideEffects: [
          "Permanently removes local episode/audio data or a queued/failed job and its checkpoints",
        ],
        cost: "No monetary cost",
        recommendedPolicy: "require_approval",
      },
      execute: (input) =>
        Effect.gen(function* () {
          if (input.resource === "episode") {
            const episode = yield* PressPodsPersistence.deleteEpisode(input.episodeId);
            if (!episode)
              return yield* Effect.fail(new Error("PressPods episode not found"));
            yield* deleteEpisodeAudio(episode.audioFile);
          } else {
            const job = yield* PressPodsPersistence.getJob(input.jobId);
            if (!job) return yield* Effect.fail(new Error("PressPods job not found"));
            if (job.status === "processing")
              return yield* Effect.fail(
                new Error("A processing PressPods job cannot be dismissed"),
              );
            yield* PressPodsPersistence.deleteJob(job.jobId);
            yield* clearChunkCheckpoints(checkpointWorkId(jobNormalizedUrl(job)));
          }
          return { resource: input.resource, deleted: true };
        }),
    }),
  ];
}
