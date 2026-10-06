import { z } from "zod";
import { Clock, Effect } from "effect";
import { getAllBriefingHistories } from "../../briefing-agent/persistence.js";
import {
  getLivestreamDiagnostics,
  getLivestreamEvents,
  getLivestreamIntelligence,
} from "../../live-check/intelligence/persistence.js";
import {
  getPlatformViewerMetrics,
  getViewerMetricsEffect,
} from "../../live-check/metrics/persistence.js";
import { getStreamerStatusEffect } from "../../live-check/persistence.js";
import { platformConfigs } from "../../live-check/platforms/index.js";
import { getStreamSessions } from "../../live-check/sessions.js";
import { fromSync } from "../../effect/interop.js";
import { getActiveRunLogs } from "../../task-runs/logCapture.js";
import { getRun, getRunLogs, getRuns } from "../../task-runs/persistence.js";
import { workspaceDefinitions } from "../../workspaces/definitions.js";
import type { McpRuntime } from "../runtime.js";
import {
  annotations,
  defineTool,
  emptyInputSchema,
  type McpToolDefinition,
  paginate,
  paginationInputShape,
  truncate,
} from "../tool.js";

const nullableString = z.string().nullable();

const nullableNumber = z.number().nullable();

const pageSchema = {
  nextCursor: z.number().int().nonnegative().nullable(),
  total: z.number().int().nonnegative(),
};

const taskRunSchema = z.object({
  runId: z.string(),
  taskName: z.string(),
  trigger: z.enum(["schedule", "manual", "startup", "catchup"]),
  scheduledFor: z.number().optional(),
  startedAt: z.number(),
  finishedAt: z.number().optional(),
  status: z.enum(["running", "success", "error"]),
  error: z.string().optional(),
  summary: z.string().optional(),
});

const taskSchema = z.object({
  name: z.string(),
  displayName: z.string().nullable(),
  schedule: z.string(),
  running: z.boolean(),
  nextRuns: z.array(z.string()),
  lastRun: taskRunSchema.nullable(),
});

const bindingSchema = z.object({
  platform: z.enum(["youtube", "twitch", "kick"]),
  username: z.string(),
  url: z.string().url(),
});

const liveSourceSchema = z.object({
  platform: z.enum(["youtube", "twitch", "kick"]),
  username: z.string(),
  title: z.string(),
  viewerCount: nullableNumber,
  category: nullableString,
});

const streamerSchema = z.object({
  id: z.string(),
  displayName: z.string(),
  tier: z.enum(["primary", "background"]),
  bindings: z.array(bindingSchema),
  dgg: z.object({ hosted: z.boolean(), viewers: nullableNumber }).nullable(),
  live: z.boolean(),
  title: nullableString,
  category: nullableString,
  viewerCount: nullableNumber,
  maxViewerCount: nullableNumber,
  startedAt: nullableNumber,
  lastStartedAt: nullableNumber,
  lastEndedAt: nullableNumber,
  primary: bindingSchema.nullable(),
  sources: z.array(liveSourceSchema),
});

const dailyBucketSchema = z.object({
  date: z.string(),
  maxViewers: z.number().int().nonnegative(),
  timestamp: z.number(),
});

const metricsSchema = z.object({
  dailyBuckets: z.array(dailyBucketSchema),
  allTimeMax: z.number().int().nonnegative(),
  allTimeMaxTimestamp: z.number(),
  platforms: z.array(
    z.object({
      platform: z.string(),
      username: z.string(),
      dailyBuckets: z.array(dailyBucketSchema),
      allTimeMax: z.number().int().nonnegative(),
      allTimeMaxTimestamp: z.number(),
    }),
  ),
});

const sessionSchema = z.object({
  startedAt: z.number(),
  endedAt: z.number(),
  durationMs: z.number().nonnegative(),
  peakViewers: z.number().int().nonnegative(),
  title: z.string(),
  platform: z.enum(["youtube", "twitch", "kick"]),
  username: z.string(),
});

const scalarMetricSchema = z.union([z.string(), z.number(), z.boolean(), z.null()]);

const stageDiagnosticSchema = z.object({
  status: z.enum(["idle", "running", "success", "skipped", "error"]),
  eligible: z.boolean().optional(),
  startedAt: z.number().optional(),
  finishedAt: z.number().optional(),
  nextAt: z.number().optional(),
  durationMs: z.number().optional(),
  detail: z.string().optional(),
  metrics: z.record(z.string(), scalarMetricSchema).optional(),
});

const intelligenceSchema = z.object({
  current: z
    .object({
      streamerId: z.string(),
      sessionStartedAt: z.number(),
      semantic: z
        .object({
          headline: z.string(),
          topics: z.array(z.string()),
          contentKind: z.enum([
            "politics",
            "debate",
            "news",
            "gaming",
            "conversation",
            "other",
          ]),
          importance: z.number(),
          reason: z.string(),
          updatedAt: z.number(),
        })
        .optional(),
      trend: z
        .object({
          percentChange: z.number(),
          viewersPerMinute: z.number(),
          dggPercentChange: nullableNumber,
          anomalous: z.boolean(),
          reason: nullableString,
          currentViewers: nullableNumber.optional(),
          baselineViewers: nullableNumber.optional(),
          currentDggViewers: nullableNumber.optional(),
          baselineDggViewers: nullableNumber.optional(),
          baselineSamples: z.number().optional(),
          candidateObservations: z.number().optional(),
          suppressionReason: nullableString.optional(),
          updatedAt: z.number(),
        })
        .optional(),
      relevanceScore: z.number(),
      relevanceReasons: z.array(z.string()),
      summary: z
        .object({
          text: z.string(),
          topic: z.string(),
          confidence: z.number(),
          transcriptExcerpt: z.string(),
          updatedAt: z.number(),
          windowSeconds: z.number(),
        })
        .optional(),
      chapters: z.array(
        z.object({
          chapterId: z.string(),
          startedAt: z.number(),
          title: z.string(),
          summary: z.string(),
        }),
      ),
      destinyPresence: z
        .object({
          state: z.enum(["possible", "confirmed"]),
          confidence: z.number(),
          detectedAt: z.number(),
          reason: z.string(),
        })
        .optional(),
      latestAlert: z
        .object({
          alertId: z.string(),
          type: z.enum([
            "destiny_guest",
            "breaking_news",
            "debate",
            "guest_joined",
            "major_announcement",
            "viewer_surge",
            "cross_stream_topic",
          ]),
          title: z.string(),
          message: z.string(),
          reason: z.string(),
          confidence: z.number(),
          createdAt: z.number(),
        })
        .optional(),
      alertedAtByType: z.record(z.string(), z.number()).optional(),
      updatedAt: z.number(),
    })
    .nullable(),
  diagnostics: z
    .object({
      streamerId: z.string(),
      sessionStartedAt: z.number().optional(),
      stages: z.object({
        metadata: stageDiagnosticSchema.optional(),
        voice: stageDiagnosticSchema.optional(),
        summary: stageDiagnosticSchema.optional(),
        alert: stageDiagnosticSchema.optional(),
      }),
      updatedAt: z.number(),
    })
    .nullable(),
  events: z.array(
    z.object({
      eventId: z.string(),
      streamerId: z.string(),
      sessionStartedAt: z.number().optional(),
      createdAt: z.number(),
      kind: z.enum([
        "session",
        "metadata",
        "voice",
        "summary",
        "alert",
        "feedback",
        "anomaly",
      ]),
      status: z.enum(["info", "success", "warning", "error"]),
      title: z.string(),
      detail: z.string().optional(),
      durationMs: z.number().optional(),
      costCents: z.number().optional(),
      metrics: z.record(z.string(), scalarMetricSchema).optional(),
    }),
  ),
  runtime: z
    .object({
      enabled: z.literal(true),
      voiceprintLoaded: z.boolean(),
      model: z.string(),
      queues: z.object({
        capture: z.object({ running: z.number(), queued: z.number() }),
        speech: z.object({ running: z.number(), queued: z.number() }),
        llm: z.object({ running: z.number(), queued: z.number() }),
      }),
      activeStreamCount: z.number().int().nonnegative(),
      activeVoiceTargetCount: z.number().int().nonnegative(),
      budget: z.object({
        spentCents: z.number(),
        limitCents: z.number(),
        remainingCents: z.number(),
      }),
      intervals: z.object({
        voiceSeconds: z.number(),
        summarySeconds: z.number(),
      }),
    })
    .nullable(),
});

const briefingNotificationSchema = z.object({
  briefingName: z.string(),
  title: z.string(),
  message: z.string(),
  messageTruncated: z.boolean(),
  url: z.string(),
  timestamp: z.number(),
  runId: nullableString,
  costCents: nullableNumber,
});

function epoch(value: Date | string | undefined): number | null {
  return value === undefined ? null : new Date(value).getTime();
}

function serializeStreamer(runtime: McpRuntime, streamerId: string) {
  const streamer = runtime.streamers.find(({ id }) => id === streamerId);
  return Effect.gen(function* () {
    if (!streamer) {
      return yield* fromSync("resolve livestream", () => {
        throw new Error(`Unknown livestream "${streamerId}"`);
      });
    }
    const status = yield* getStreamerStatusEffect(streamer.id);
    const bindings = streamer.bindings.map((binding) => ({
      platform: binding.platform,
      username: binding.username,
      url:
        binding.urlOverride ??
        platformConfigs[binding.platform].getLiveUrl(binding.username),
    }));
    if (!status.isLive) {
      return {
        id: streamer.id,
        displayName: streamer.displayName,
        tier: streamer.tier,
        bindings,
        dgg: streamer.dgg ?? null,
        live: false as const,
        title: null,
        category: null,
        viewerCount: null,
        maxViewerCount: status.lastMaxViewerCount ?? null,
        startedAt: null,
        lastStartedAt: epoch(status.lastStartedAt),
        lastEndedAt: epoch(status.lastEndedAt),
        primary: null,
        sources: [],
      };
    }
    const primary = {
      platform: status.primary.platform,
      username: status.primary.username,
      url:
        status.primary.urlOverride ??
        platformConfigs[status.primary.platform].getLiveUrl(status.primary.username),
    };
    return {
      id: streamer.id,
      displayName: streamer.displayName,
      tier: streamer.tier,
      bindings,
      dgg: streamer.dgg ?? null,
      live: true as const,
      title: status.primaryTitle,
      category: status.category ?? null,
      viewerCount: status.viewerCount ?? null,
      maxViewerCount: status.maxViewerCount,
      startedAt: epoch(status.startedAt),
      lastStartedAt: null,
      lastEndedAt: null,
      primary,
      sources: (status.sources ?? []).map((source) => ({
        platform: source.platform,
        username: source.username,
        title: source.title,
        viewerCount: source.viewerCount ?? null,
        category: source.category ?? null,
      })),
    };
  });
}

export function createSystemTools(runtime: McpRuntime): McpToolDefinition[] {
  return [
    defineTool({
      name: "system_status",
      title: "System Status",
      description:
        "Report which high-level Omni capabilities are configured without exposing account identifiers, credentials, configuration values, or host details.",
      inputSchema: emptyInputSchema,
      outputSchema: z.object({
        capabilities: z.object({
          taskControls: z.boolean(),
          livestreams: z.boolean(),
          livestreamIntelligence: z.boolean(),
          briefings: z.boolean(),
          iCloudEmail: z.boolean(),
          iCloudCalendar: z.boolean(),
          webSearch: z.boolean(),
          iosControls: z.boolean(),
          printing: z.boolean(),
          workspaces: z.boolean(),
        }),
      }),
      annotations: annotations(true, false, true, false),
      policy: { sideEffects: [], cost: "none", recommendedPolicy: "allow" },
      execute: () =>
        Effect.gen(function* () {
          const taskNames = new Set(
            (yield* runtime.registry.list()).map(({ name }) => name),
          );
          const iCloudConfigured = Boolean(
            process.env.ICLOUD_USERNAME && process.env.ICLOUD_APP_PASSWORD,
          );
          return {
            capabilities: {
              taskControls: taskNames.size > 0,
              livestreams: runtime.streamers.length > 0,
              livestreamIntelligence: runtime.livestreamDiagnostics !== undefined,
              briefings: Boolean(process.env.BRIEFINGS_PATH),
              iCloudEmail:
                runtime.emailControls.transport !== undefined && iCloudConfigured,
              iCloudCalendar: iCloudConfigured,
              webSearch: Boolean(process.env.TAVILY_API_KEY),
              iosControls: runtime.iosControls !== undefined,
              printing: runtime.printer !== undefined,
              workspaces: workspaceDefinitions.some(({ taskName }) =>
                taskNames.has(taskName),
              ),
            },
          };
        }),
    }),
    defineTool({
      name: "tasks_list",
      title: "List Tasks",
      description:
        "List registered Omni tasks with schedules, running state, upcoming executions, and the latest recorded run.",
      inputSchema: z.object({ ...paginationInputShape }).strict(),
      outputSchema: z.object({ tasks: z.array(taskSchema), ...pageSchema }),
      annotations: annotations(true, false, true, false),
      policy: { sideEffects: [], cost: "none", recommendedPolicy: "allow" },
      execute: ({ cursor, limit }) =>
        Effect.gen(function* () {
          const page = paginate(yield* runtime.registry.list(), cursor, limit);
          return {
            tasks: page.items.map((task) => ({
              ...task,
              displayName: task.displayName ?? null,
            })),
            nextCursor: page.nextCursor,
            total: page.total,
          };
        }),
    }),
    defineTool({
      name: "task_run",
      title: "Run Task",
      description:
        "Queue one registered task for immediate execution. The task may send notifications, call paid services, modify external systems, or perform other consequential work; inspect the task and obtain approval first.",
      inputSchema: z
        .object({
          taskName: z.string().trim().min(1).max(200),
          input: z
            .record(z.string(), z.unknown())
            .optional()
            .describe("Optional task-specific manual input"),
        })
        .strict(),
      outputSchema: z.object({
        runId: z.string(),
        taskName: z.string(),
        queued: z.literal(true),
      }),
      annotations: annotations(false, false, false, true),
      policy: {
        sideEffects: [
          "Queues task execution",
          "Effects depend on the selected task and may include external communications or external mutations",
        ],
        cost: "task-dependent; some tasks invoke paid AI, search, notification, or media services",
        recommendedPolicy: "require_approval",
      },
      execute: ({ taskName, input }) =>
        runtime.registry.runNow(taskName, input).pipe(
          Effect.map((run) => ({
            ...run,
            taskName,
            queued: true as const,
          })),
        ),
    }),
    defineTool({
      name: "task_runs_list",
      title: "List Task Runs",
      description:
        "List recent persisted task runs, optionally filtered by exact task name. Results are newest first and bounded to the newest 500 runs.",
      inputSchema: z
        .object({
          taskName: z.string().trim().min(1).max(200).optional(),
          cursor: z.number().int().min(0).max(499).default(0),
          limit: z.number().int().min(1).max(100).default(25),
        })
        .strict(),
      outputSchema: z.object({
        runs: z.array(taskRunSchema),
        ...pageSchema,
        resultWindowTruncated: z.boolean(),
      }),
      annotations: annotations(true, false, true, false),
      policy: { sideEffects: [], cost: "none", recommendedPolicy: "allow" },
      execute: ({ taskName, cursor, limit }) =>
        Effect.gen(function* () {
          const runs = yield* getRuns(taskName, 501);
          const truncatedWindow = runs.length > 500;
          const page = paginate(runs.slice(0, 500), cursor, limit);
          return {
            runs: page.items,
            nextCursor: page.nextCursor,
            total: page.total,
            resultWindowTruncated: truncatedWindow,
          };
        }),
    }),
    defineTool({
      name: "task_run_get",
      title: "Get Task Run",
      description:
        "Get one task run and a bounded page of its captured logs. Log messages are truncated to prevent oversized results.",
      inputSchema: z
        .object({
          runId: z.string().trim().min(1).max(300),
          logCursor: z.number().int().min(0).max(19_999).default(0),
          logLimit: z.number().int().min(1).max(200).default(100),
          maxMessageChars: z.number().int().min(100).max(4_000).default(2_000),
        })
        .strict(),
      outputSchema: z.object({
        run: taskRunSchema,
        logs: z.array(
          z.object({
            timestamp: z.number(),
            level: z.string(),
            logger: z.string(),
            message: z.string(),
            messageTruncated: z.boolean(),
          }),
        ),
        logNextCursor: z.number().int().nonnegative().nullable(),
        logTotal: z.number().int().nonnegative(),
        droppedLogs: z.number().int().nonnegative(),
      }),
      annotations: annotations(true, false, true, false),
      policy: { sideEffects: [], cost: "none", recommendedPolicy: "allow" },
      execute: ({ runId, logCursor, logLimit, maxMessageChars }) =>
        Effect.gen(function* () {
          const run = yield* getRun(runId);
          if (!run) throw new Error(`Unknown task run "${runId}"`);
          const stored =
            getActiveRunLogs(runId) ?? (yield* getRunLogs(runId, runtime.logger));
          const lines = stored?.lines ?? [];
          const page = paginate(lines, logCursor, logLimit);
          return {
            run,
            logs: page.items.map((line) => {
              const message = truncate(line.msg, maxMessageChars);
              return {
                timestamp: line.t,
                level: line.level,
                logger: line.logger,
                message: message.text,
                messageTruncated: message.truncated,
              };
            }),
            logNextCursor: page.nextCursor,
            logTotal: page.total,
            droppedLogs: stored?.dropped ?? 0,
          };
        }),
    }),
    defineTool({
      name: "livestreams_list",
      title: "List Livestreams",
      description:
        "List configured streamer identities and their current persisted live state without polling external platforms.",
      inputSchema: z
        .object({
          liveOnly: z.boolean().default(false),
          ...paginationInputShape,
        })
        .strict(),
      outputSchema: z.object({ livestreams: z.array(streamerSchema), ...pageSchema }),
      annotations: annotations(true, false, true, false),
      policy: { sideEffects: [], cost: "none", recommendedPolicy: "allow" },
      execute: ({ liveOnly, cursor, limit }) =>
        Effect.gen(function* () {
          const values = (yield* Effect.forEach(runtime.streamers, ({ id }) =>
            serializeStreamer(runtime, id),
          ))
            .filter(({ live }) => !liveOnly || live)
            .sort(
              (a, b) =>
                Number(b.live) - Number(a.live) ||
                (b.viewerCount ?? 0) - (a.viewerCount ?? 0) ||
                a.displayName.localeCompare(b.displayName),
            );
          const page = paginate(values, cursor, limit);
          return {
            livestreams: page.items,
            nextCursor: page.nextCursor,
            total: page.total,
          };
        }),
    }),
    defineTool({
      name: "livestream_get",
      title: "Get Livestream",
      description:
        "Get one streamer's persisted live state, with optional bounded viewer metrics, completed sessions, and intelligence diagnostics. This does not poll the platforms.",
      inputSchema: z
        .object({
          streamerId: z.string().trim().min(1).max(200),
          include: z
            .array(z.enum(["metrics", "sessions", "intelligence"]))
            .max(3)
            .default([]),
          metricsDays: z.number().int().min(1).max(180).default(30),
          sessionLimit: z.number().int().min(1).max(100).default(20),
          intelligenceEventLimit: z.number().int().min(1).max(100).default(25),
        })
        .strict(),
      outputSchema: z.object({
        livestream: streamerSchema,
        metrics: metricsSchema.nullable(),
        sessions: z.array(sessionSchema).nullable(),
        intelligence: intelligenceSchema.nullable(),
      }),
      annotations: annotations(true, false, true, false),
      policy: { sideEffects: [], cost: "none", recommendedPolicy: "allow" },
      execute: ({
        streamerId,
        include,
        metricsDays,
        sessionLimit,
        intelligenceEventLimit,
      }) =>
        Effect.gen(function* () {
          const requested = new Set(include);
          const cutoff = (yield* Clock.currentTimeMillis) - metricsDays * 86_400_000;
          const aggregate = requested.has("metrics")
            ? yield* getViewerMetricsEffect(streamerId)
            : undefined;
          const platformMetrics = aggregate
            ? yield* getPlatformViewerMetrics(streamerId)
            : [];
          const metrics = aggregate
            ? {
                dailyBuckets: aggregate.dailyBuckets.filter(
                  ({ timestamp }) => timestamp >= cutoff,
                ),
                allTimeMax: aggregate.allTimeMax,
                allTimeMaxTimestamp: aggregate.allTimeMaxTimestamp,
                platforms: platformMetrics.map((item) => ({
                  platform: item.platform,
                  username: item.username,
                  dailyBuckets: item.dailyBuckets.filter(
                    ({ timestamp }) => timestamp >= cutoff,
                  ),
                  allTimeMax: item.allTimeMax,
                  allTimeMaxTimestamp: item.allTimeMaxTimestamp,
                })),
              }
            : null;
          const sessions = requested.has("sessions")
            ? [...(yield* getStreamSessions(streamerId)).sessions]
                .sort((a, b) => b.endedAt - a.endedAt)
                .slice(0, sessionLimit)
            : null;
          return {
            livestream: yield* serializeStreamer(runtime, streamerId),
            metrics,
            sessions,
            intelligence: requested.has("intelligence")
              ? {
                  current: getLivestreamIntelligence(streamerId) ?? null,
                  diagnostics: getLivestreamDiagnostics(streamerId) ?? null,
                  events: getLivestreamEvents(streamerId, intelligenceEventLimit),
                  runtime:
                    runtime.livestreamDiagnostics?.getRuntimeDiagnostics() ?? null,
                }
              : null,
          };
        }),
    }),
    defineTool({
      name: "briefings_list",
      title: "List Briefings",
      description:
        "List a bounded, newest-first page of stored briefing notifications, optionally filtered by exact briefing name.",
      inputSchema: z
        .object({
          briefingName: z.string().trim().min(1).max(200).optional(),
          cursor: z.number().int().min(0).max(4_999).default(0),
          limit: z.number().int().min(1).max(100).default(25),
          maxMessageChars: z.number().int().min(100).max(4_000).default(1_500),
        })
        .strict(),
      outputSchema: z.object({
        briefingNames: z.array(z.string()),
        notifications: z.array(briefingNotificationSchema),
        ...pageSchema,
      }),
      annotations: annotations(true, false, true, false),
      policy: { sideEffects: [], cost: "none", recommendedPolicy: "allow" },
      execute: ({ briefingName, cursor, limit, maxMessageChars }) =>
        Effect.gen(function* () {
          const histories = yield* getAllBriefingHistories();
          const names = histories.map(({ briefingName: name }) => name).sort();
          const notifications = histories
            .filter((history) => !briefingName || history.briefingName === briefingName)
            .flatMap((history) =>
              history.notifications.map((item) => ({
                history: history.briefingName,
                item,
              })),
            )
            .sort((a, b) => b.item.timestamp - a.item.timestamp);
          const page = paginate(notifications, cursor, limit);
          return {
            briefingNames: names,
            notifications: page.items.map(({ history, item }) => {
              const message = truncate(item.message, maxMessageChars);
              return {
                briefingName: history,
                title: item.title,
                message: message.text,
                messageTruncated: message.truncated,
                url: item.url,
                timestamp: item.timestamp,
                runId: item.runId ?? null,
                costCents: item.costCents ?? null,
              };
            }),
            nextCursor: page.nextCursor,
            total: page.total,
          };
        }),
    }),
  ];
}
