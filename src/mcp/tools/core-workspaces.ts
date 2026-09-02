import { z } from "zod";
import { Clock, Effect, Exit, Schema } from "effect";
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
import {
  approveWorkspaceActionEffect,
  rejectWorkspaceActionEffect,
} from "../../workspaces/actions.js";
import {
  getWorkspaceDefinition,
  workspaceDefinitions,
} from "../../workspaces/definitions.js";
import {
  getLatestWorkspaceArtifacts,
  getWorkspaceEmailScope,
  getWorkspaceSubject,
  listWorkspaceActions,
  listWorkspaceArtifactRevisions,
  listWorkspaceMessages,
  listWorkspacePapercuts,
  listWorkspaceSources,
  listWorkspaceSubjects,
  resolveWorkspacePapercut,
  upsertWorkspaceSubject,
  type WorkspaceActionData,
  type WorkspaceArtifactRevisionData,
  type WorkspacePapercutData,
} from "../../workspaces/persistence.js";
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

const workspaceSummarySchema = z.object({
  id: z.string(),
  title: z.string(),
  description: z.string(),
  subjectLabel: z.string(),
  subjectLabelPlural: z.string(),
  scheduledRuns: z.boolean(),
  artifacts: z.array(
    z.object({
      key: z.string(),
      title: z.string(),
      kind: z.enum([
        "markdown",
        "structured",
        "evidence-ledger",
        "timeline",
        "collection",
      ]),
    }),
  ),
  activeSubjectCount: z.number().int().nonnegative(),
  pendingActionCount: z.number().int().nonnegative(),
  openPapercutCount: z.number().int().nonnegative(),
});

const workspaceSubjectSchema = z.object({
  workspaceId: z.string(),
  subjectId: z.string(),
  title: z.string(),
  status: z.enum(["active", "paused", "completed", "archived"]),
  summary: z.string(),
  createdAt: z.number(),
  updatedAt: z.number(),
  lastResearchedAt: z.number().optional(),
});

const workspaceArtifactSchema = z.object({
  revisionId: z.string(),
  workspaceId: z.string(),
  subjectId: z.string(),
  artifactKey: z.string(),
  kind: z.enum(["markdown", "structured", "evidence-ledger", "timeline", "collection"]),
  content: z.string(),
  contentTruncated: z.boolean(),
  summary: z.string(),
  createdAt: z.number(),
  runId: nullableString,
});

const workspaceMessageSchema = z.object({
  messageId: z.string(),
  workspaceId: z.string(),
  subjectId: nullableString,
  role: z.enum(["user", "assistant", "system"]),
  text: z.string(),
  textTruncated: z.boolean(),
  createdAt: z.number(),
  runId: nullableString,
});

const workspaceSourceSchema = z.object({
  sourceId: z.string(),
  workspaceId: z.string(),
  subjectId: z.string(),
  kind: z.enum(["web", "email"]),
  title: z.string(),
  url: nullableString,
  excerpt: z.string(),
  excerptTruncated: z.boolean(),
  emailId: nullableString,
  createdAt: z.number(),
  runId: nullableString,
});

const workspaceActionPayloadSchema = z.union([
  z.object({
    senders: z.array(z.string()),
    domains: z.array(z.string()),
    subjectKeywords: z.array(z.string()),
    bodyKeywords: z.array(z.string()),
  }),
  z.object({
    title: z.string(),
    startDate: z.string(),
    endDate: z.string().optional(),
    startTime: z.string().optional(),
    endTime: z.string().optional(),
    location: z.string().optional(),
    description: z.string().optional(),
    timeZone: z.string().optional(),
    allDay: z.boolean(),
    reminderMinutes: z.number().optional(),
  }),
  z.object({ unavailable: z.literal("Stored action payload is invalid") }),
]);

const workspaceActionPayloadEffectSchema = Schema.Union([
  Schema.Struct({
    senders: Schema.Array(Schema.String),
    domains: Schema.Array(Schema.String),
    subjectKeywords: Schema.Array(Schema.String),
    bodyKeywords: Schema.Array(Schema.String),
  }),
  Schema.Struct({
    title: Schema.String,
    startDate: Schema.String,
    endDate: Schema.optional(Schema.String),
    startTime: Schema.optional(Schema.String),
    endTime: Schema.optional(Schema.String),
    location: Schema.optional(Schema.String),
    description: Schema.optional(Schema.String),
    timeZone: Schema.optional(Schema.String),
    allDay: Schema.Boolean,
    reminderMinutes: Schema.optional(Schema.Number),
  }),
]);

const workspaceActionSchema = z.object({
  actionId: z.string(),
  workspaceId: z.string(),
  subjectId: z.string(),
  type: z.enum(["email_scope", "calendar_event"]),
  status: z.enum(["pending", "approved", "rejected", "failed"]),
  title: z.string(),
  description: z.string(),
  payload: workspaceActionPayloadSchema,
  createdAt: z.number(),
  resolvedAt: z.number().optional(),
  result: z.string().optional(),
  runId: nullableString,
});

const workspacePapercutSchema = z.object({
  papercutId: z.string(),
  workspaceId: z.string(),
  subjectId: nullableString,
  runId: nullableString,
  category: z.enum([
    "missing-capability",
    "poor-source-data",
    "integration-friction",
    "workflow-gap",
    "prompt-problem",
    "ui-gap",
  ]),
  title: z.string(),
  detail: z.string(),
  relatedTool: nullableString,
  occurrences: z.number().int().positive(),
  firstSeenAt: z.number(),
  lastSeenAt: z.number(),
  status: z.enum(["open", "addressed", "dismissed"]),
  resolution: z.string().optional(),
});

function epoch(value: Date | string | undefined): number | null {
  return value === undefined ? null : new Date(value).getTime();
}

function requireWorkspace(workspaceId: string) {
  const definition = getWorkspaceDefinition(workspaceId);
  if (!definition) throw new Error(`Unknown workspace "${workspaceId}"`);
  return definition;
}

const requireSubject = Effect.fn("Mcp.requireWorkspaceSubject")(function* (
  workspaceId: string,
  subjectId: string,
) {
  requireWorkspace(workspaceId);
  const subject = yield* getWorkspaceSubject(workspaceId, subjectId);
  if (!subject) {
    throw new Error(`Unknown subject "${subjectId}" in workspace "${workspaceId}"`);
  }
  return subject;
});

const serializeWorkspaceDefinition = Effect.fn("Mcp.serializeWorkspaceDefinition")(
  function* (definition: (typeof workspaceDefinitions)[number]) {
    const subjects = yield* listWorkspaceSubjects(definition.id);
    return {
      id: definition.id,
      title: definition.title,
      description: definition.description,
      subjectLabel: definition.subjectLabel,
      subjectLabelPlural: definition.subjectLabelPlural,
      scheduledRuns: definition.scheduledRuns !== false,
      artifacts: definition.artifacts.map(({ key, title, kind }) => ({
        key,
        title,
        kind,
      })),
      activeSubjectCount: subjects.filter(({ status }) => status === "active").length,
      pendingActionCount: (yield* listWorkspaceActions(definition.id)).filter(
        ({ status }) => status === "pending",
      ).length,
      openPapercutCount: (yield* listWorkspacePapercuts(definition.id, "open")).length,
    };
  },
);

function parseActionPayload(payload: string): unknown {
  const decoded = Schema.decodeUnknownExit(
    Schema.fromJsonString(workspaceActionPayloadEffectSchema),
  )(payload);
  return Exit.isSuccess(decoded)
    ? decoded.value
    : { unavailable: "Stored action payload is invalid" };
}

function searchSnippet(value: string, query: string, maxChars: number): string {
  if (value.length <= maxChars) return value;
  const index = value.toLocaleLowerCase().indexOf(query.toLocaleLowerCase());
  if (index < 0) return truncate(value, maxChars).text;
  const start = Math.max(0, index - Math.floor((maxChars - query.length) / 2));
  const end = Math.min(value.length, start + maxChars);
  return `${start > 0 ? "…" : ""}${value.slice(start, end)}${end < value.length ? "…" : ""}`;
}

function serializeAction(action: WorkspaceActionData) {
  return {
    ...action,
    payload: parseActionPayload(action.payload),
    runId: action.runId ?? null,
  };
}

function serializePapercut(papercut: WorkspacePapercutData) {
  const { fingerprint: _fingerprint, ...safe } = papercut;
  return {
    ...safe,
    subjectId: safe.subjectId ?? null,
    runId: safe.runId ?? null,
    relatedTool: safe.relatedTool ?? null,
  };
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

const workspaceSearchMatches = Effect.fn("Mcp.workspaceSearchMatches")(
  function* (input: { workspaceId?: string; query: string; maxSnippetChars: number }) {
    const query = input.query.toLocaleLowerCase();
    const matches: Array<{
      workspaceId: string;
      subjectId: string;
      resourceType: "subject" | "artifact" | "message" | "source";
      resourceId: string;
      title: string;
      snippet: string;
      updatedAt: number;
    }> = [];
    const definitions = input.workspaceId
      ? [requireWorkspace(input.workspaceId)]
      : workspaceDefinitions;
    const add = (value: (typeof matches)[number], haystack: string) => {
      if (haystack.toLocaleLowerCase().includes(query)) {
        matches.push({
          ...value,
          snippet: searchSnippet(haystack, input.query, input.maxSnippetChars),
        });
      }
    };
    for (const definition of definitions) {
      for (const subject of yield* listWorkspaceSubjects(definition.id)) {
        add(
          {
            workspaceId: definition.id,
            subjectId: subject.subjectId,
            resourceType: "subject",
            resourceId: subject.subjectId,
            title: subject.title,
            snippet: truncate(subject.summary, input.maxSnippetChars).text,
            updatedAt: subject.updatedAt,
          },
          `${subject.title}\n${subject.summary}`,
        );
        for (const artifact of yield* getLatestWorkspaceArtifacts(
          definition.id,
          subject.subjectId,
        )) {
          add(
            {
              workspaceId: definition.id,
              subjectId: subject.subjectId,
              resourceType: "artifact",
              resourceId: artifact.revisionId,
              title: artifact.artifactKey,
              snippet: truncate(artifact.content, input.maxSnippetChars).text,
              updatedAt: artifact.createdAt,
            },
            `${artifact.summary}\n${artifact.content}`,
          );
        }
        for (const message of yield* listWorkspaceMessages(
          definition.id,
          subject.subjectId,
          100,
        )) {
          add(
            {
              workspaceId: definition.id,
              subjectId: subject.subjectId,
              resourceType: "message",
              resourceId: message.messageId,
              title: `${message.role} message`,
              snippet: truncate(message.text, input.maxSnippetChars).text,
              updatedAt: message.createdAt,
            },
            message.text,
          );
        }
        for (const source of yield* listWorkspaceSources(
          definition.id,
          subject.subjectId,
          100,
        )) {
          add(
            {
              workspaceId: definition.id,
              subjectId: subject.subjectId,
              resourceType: "source",
              resourceId: source.sourceId,
              title: source.title,
              snippet: truncate(source.excerpt, input.maxSnippetChars).text,
              updatedAt: source.createdAt,
            },
            `${source.title}\n${source.excerpt}`,
          );
        }
      }
    }
    return matches.sort((a, b) => b.updatedAt - a.updatedAt);
  },
);

export function createCoreWorkspaceTools(runtime: McpRuntime): McpToolDefinition[] {
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
    defineTool({
      name: "workspaces_list",
      title: "List Workspaces",
      description:
        "List Omni's durable personal workspaces with compact definitions and current subject, approval, and papercut counts. Internal agent prompts are excluded.",
      inputSchema: emptyInputSchema,
      outputSchema: z.object({ workspaces: z.array(workspaceSummarySchema) }),
      annotations: annotations(true, false, true, false),
      policy: { sideEffects: [], cost: "none", recommendedPolicy: "allow" },
      execute: () =>
        Effect.gen(function* () {
          return {
            workspaces: yield* Effect.forEach(
              workspaceDefinitions,
              serializeWorkspaceDefinition,
            ),
          };
        }),
    }),
    defineTool({
      name: "workspace_get",
      title: "Get Workspace",
      description:
        "Get a workspace overview or one subject dossier. Subject results include bounded current artifacts, messages, sources, actions, email scope, and open papercuts.",
      inputSchema: z
        .object({
          workspaceId: z.string().trim().min(1).max(100),
          subjectId: z.string().trim().min(1).max(200).optional(),
          messageLimit: z.number().int().min(1).max(100).default(30),
          sourceLimit: z.number().int().min(1).max(100).default(30),
          revisionLimit: z.number().int().min(1).max(100).default(30),
          actionLimit: z.number().int().min(1).max(100).default(30),
          maxContentChars: z.number().int().min(200).max(10_000).default(4_000),
        })
        .strict(),
      outputSchema: z.object({
        workspace: workspaceSummarySchema,
        subjects: z.array(workspaceSubjectSchema),
        subjectsTruncated: z.boolean(),
        subject: workspaceSubjectSchema.nullable(),
        artifacts: z.array(workspaceArtifactSchema),
        artifactRevisions: z.array(workspaceArtifactSchema),
        messages: z.array(workspaceMessageSchema),
        sources: z.array(workspaceSourceSchema),
        actions: z.array(workspaceActionSchema),
        emailScope: z
          .object({
            senders: z.array(z.string()),
            domains: z.array(z.string()),
            subjectKeywords: z.array(z.string()),
            bodyKeywords: z.array(z.string()),
            updatedAt: z.number(),
          })
          .nullable(),
        papercuts: z.array(workspacePapercutSchema),
        papercutsTruncated: z.boolean(),
      }),
      annotations: annotations(true, false, true, false),
      policy: { sideEffects: [], cost: "none", recommendedPolicy: "allow" },
      execute: ({
        workspaceId,
        subjectId,
        messageLimit,
        sourceLimit,
        revisionLimit,
        actionLimit,
        maxContentChars,
      }) =>
        Effect.gen(function* () {
          const definition = requireWorkspace(workspaceId);
          const subject = subjectId
            ? yield* requireSubject(workspaceId, subjectId)
            : undefined;
          const serializeArtifact = (item: WorkspaceArtifactRevisionData) => {
            const content = truncate(item.content, maxContentChars);
            return {
              ...item,
              content: content.text,
              contentTruncated: content.truncated,
              runId: item.runId ?? null,
            };
          };
          const messages = subjectId
            ? (yield* listWorkspaceMessages(workspaceId, subjectId, messageLimit)).map(
                (item) => {
                  const text = truncate(item.text, maxContentChars);
                  return {
                    ...item,
                    subjectId: item.subjectId ?? null,
                    text: text.text,
                    textTruncated: text.truncated,
                    runId: item.runId ?? null,
                  };
                },
              )
            : [];
          const sources = subjectId
            ? (yield* listWorkspaceSources(workspaceId, subjectId, sourceLimit)).map(
                (item) => {
                  const excerpt = truncate(item.excerpt, maxContentChars);
                  return {
                    ...item,
                    url: item.url ?? null,
                    excerpt: excerpt.text,
                    excerptTruncated: excerpt.truncated,
                    emailId: item.emailId ?? null,
                    runId: item.runId ?? null,
                  };
                },
              )
            : [];
          const scope = subjectId
            ? yield* getWorkspaceEmailScope(workspaceId, subjectId)
            : undefined;
          const subjects = yield* listWorkspaceSubjects(workspaceId);
          const papercuts = (yield* listWorkspacePapercuts(workspaceId, "open")).filter(
            (item) => !subjectId || !item.subjectId || item.subjectId === subjectId,
          );
          const artifacts = subjectId
            ? (yield* getLatestWorkspaceArtifacts(workspaceId, subjectId)).map(
                serializeArtifact,
              )
            : [];
          const artifactRevisions = subjectId
            ? (yield* listWorkspaceArtifactRevisions(workspaceId, subjectId))
                .slice(0, revisionLimit)
                .map(serializeArtifact)
            : [];
          const actions = (yield* listWorkspaceActions(workspaceId, subjectId))
            .slice(0, actionLimit)
            .map(serializeAction);
          return {
            workspace: yield* serializeWorkspaceDefinition(definition),
            subjects: subjects.slice(0, 100),
            subjectsTruncated: subjects.length > 100,
            subject: subject ?? null,
            artifacts,
            artifactRevisions,
            messages,
            sources,
            actions,
            emailScope: scope
              ? {
                  senders: scope.senders,
                  domains: scope.domains,
                  subjectKeywords: scope.subjectKeywords,
                  bodyKeywords: scope.bodyKeywords,
                  updatedAt: scope.updatedAt,
                }
              : null,
            papercuts: papercuts.slice(0, 100).map(serializePapercut),
            papercutsTruncated: papercuts.length > 100,
          };
        }),
    }),
    defineTool({
      name: "workspace_search",
      title: "Search Workspaces",
      description:
        "Search subject titles and summaries, current artifacts, recent messages, and recent sources across one or all workspaces. Results contain bounded snippets.",
      inputSchema: z
        .object({
          query: z.string().trim().min(2).max(300),
          workspaceId: z.string().trim().min(1).max(100).optional(),
          cursor: z.number().int().min(0).max(4_999).default(0),
          limit: z.number().int().min(1).max(100).default(25),
          maxSnippetChars: z.number().int().min(100).max(1_000).default(400),
        })
        .strict(),
      outputSchema: z.object({
        matches: z.array(
          z.object({
            workspaceId: z.string(),
            subjectId: z.string(),
            resourceType: z.enum(["subject", "artifact", "message", "source"]),
            resourceId: z.string(),
            title: z.string(),
            snippet: z.string(),
            updatedAt: z.number(),
          }),
        ),
        ...pageSchema,
      }),
      annotations: annotations(true, false, true, false),
      policy: { sideEffects: [], cost: "none", recommendedPolicy: "allow" },
      execute: ({ query, workspaceId, cursor, limit, maxSnippetChars }) =>
        Effect.gen(function* () {
          const page = paginate(
            yield* workspaceSearchMatches({ workspaceId, query, maxSnippetChars }),
            cursor,
            limit,
          );
          return {
            matches: page.items,
            nextCursor: page.nextCursor,
            total: page.total,
          };
        }),
    }),
    defineTool({
      name: "workspace_message",
      title: "Message Workspace",
      description:
        "Send a user message to a workspace agent, optionally continuing an existing subject. This queues paid model and web-research work and may create reviewable action proposals, but does not approve them.",
      inputSchema: z
        .object({
          workspaceId: z.string().trim().min(1).max(100),
          subjectId: z.string().trim().min(1).max(200).optional(),
          message: z.string().trim().min(1).max(20_000),
        })
        .strict(),
      outputSchema: z.object({
        workspaceId: z.string(),
        subjectId: nullableString,
        runId: z.string(),
        queued: z.literal(true),
      }),
      annotations: annotations(false, false, false, true),
      policy: {
        sideEffects: [
          "Queues a workspace agent run",
          "Persists the message and resulting dossier revisions, sources, and proposals",
        ],
        cost: "variable paid model and web-search cost",
        recommendedPolicy: "require_approval",
      },
      execute: ({ workspaceId, subjectId, message }) =>
        Effect.gen(function* () {
          const definition = requireWorkspace(workspaceId);
          if (subjectId) yield* requireSubject(workspaceId, subjectId);
          const run = yield* runtime.registry.runNow(definition.taskName, {
            message,
            subjectId,
          });
          return {
            workspaceId,
            subjectId: subjectId ?? null,
            runId: run.runId,
            queued: true as const,
          };
        }),
    }),
    defineTool({
      name: "workspace_subject_set_status",
      title: "Set Workspace Subject Status",
      description:
        "Set a workspace subject to active, paused, completed, or archived. This is a local preference/state change and does not run the workspace agent.",
      inputSchema: z
        .object({
          workspaceId: z.string().trim().min(1).max(100),
          subjectId: z.string().trim().min(1).max(200),
          status: z.enum(["active", "paused", "completed", "archived"]),
        })
        .strict(),
      outputSchema: z.object({ subject: workspaceSubjectSchema }),
      annotations: annotations(false, false, true, false),
      policy: {
        sideEffects: ["Updates local workspace subject state"],
        cost: "none",
        recommendedPolicy: "allow",
      },
      execute: ({ workspaceId, subjectId, status }) =>
        Effect.gen(function* () {
          const subject = yield* requireSubject(workspaceId, subjectId);
          return {
            subject: yield* upsertWorkspaceSubject({
              ...subject,
              status,
            }),
          };
        }),
    }),
    defineTool({
      name: "workspace_actions_list",
      title: "List Workspace Actions",
      description:
        "List reviewable workspace action proposals, optionally filtered by workspace, subject, or status. Payloads are parsed into typed JSON-compatible values.",
      inputSchema: z
        .object({
          workspaceId: z.string().trim().min(1).max(100).optional(),
          subjectId: z.string().trim().min(1).max(200).optional(),
          status: z.enum(["pending", "approved", "rejected", "failed"]).optional(),
          cursor: z.number().int().min(0).max(4_999).default(0),
          limit: z.number().int().min(1).max(100).default(25),
        })
        .strict(),
      outputSchema: z.object({
        actions: z.array(workspaceActionSchema),
        ...pageSchema,
      }),
      annotations: annotations(true, false, true, false),
      policy: { sideEffects: [], cost: "none", recommendedPolicy: "allow" },
      execute: ({ workspaceId, subjectId, status, cursor, limit }) =>
        Effect.gen(function* () {
          if (subjectId && !workspaceId) {
            throw new Error("workspaceId is required when subjectId is provided");
          }
          const definitions = workspaceId
            ? [requireWorkspace(workspaceId)]
            : workspaceDefinitions;
          const values = (yield* Effect.forEach(definitions, (definition) =>
            listWorkspaceActions(definition.id, subjectId),
          ))
            .flat()
            .filter((action) => !status || action.status === status)
            .sort((a, b) => b.createdAt - a.createdAt);
          const page = paginate(values, cursor, limit);
          return {
            actions: page.items.map(serializeAction),
            nextCursor: page.nextCursor,
            total: page.total,
          };
        }),
    }),
    defineTool({
      name: "workspace_action_approve",
      title: "Approve Workspace Action",
      description:
        "Approve and execute one pending or failed workspace proposal. Email-scope approvals broaden local email ingestion; calendar-event approvals write to the configured external CalDAV calendar. Executor approval is required.",
      inputSchema: z.object({ actionId: z.string().uuid() }).strict(),
      outputSchema: z.object({ action: workspaceActionSchema }),
      annotations: annotations(false, false, true, true),
      policy: {
        sideEffects: [
          "Approves a durable workspace proposal",
          "May broaden email ingestion scope or create an external calendar event",
        ],
        cost: "no direct monetary cost; may perform a CalDAV network request",
        recommendedPolicy: "require_approval",
      },
      execute: ({ actionId }) =>
        approveWorkspaceActionEffect(actionId, runtime.logger).pipe(
          Effect.map((action) => ({ action: serializeAction(action) })),
        ),
    }),
    defineTool({
      name: "workspace_action_reject",
      title: "Reject Workspace Action",
      description:
        "Reject one pending workspace proposal without performing its proposed external effect.",
      inputSchema: z.object({ actionId: z.string().uuid() }).strict(),
      outputSchema: z.object({ action: workspaceActionSchema }),
      annotations: annotations(false, false, true, false),
      policy: {
        sideEffects: ["Marks a pending local action proposal rejected"],
        cost: "none",
        recommendedPolicy: "allow",
      },
      execute: ({ actionId }) =>
        rejectWorkspaceActionEffect(actionId).pipe(
          Effect.map((action) => ({ action: serializeAction(action) })),
        ),
    }),
    defineTool({
      name: "workspace_papercuts_list",
      title: "List Workspace Papercuts",
      description:
        "List structured workspace friction reports, optionally filtered by workspace and resolution status. Internal deduplication fingerprints are excluded.",
      inputSchema: z
        .object({
          workspaceId: z.string().trim().min(1).max(100).optional(),
          status: z.enum(["open", "addressed", "dismissed"]).optional(),
          cursor: z.number().int().min(0).max(4_999).default(0),
          limit: z.number().int().min(1).max(100).default(25),
        })
        .strict(),
      outputSchema: z.object({
        papercuts: z.array(workspacePapercutSchema),
        ...pageSchema,
      }),
      annotations: annotations(true, false, true, false),
      policy: { sideEffects: [], cost: "none", recommendedPolicy: "allow" },
      execute: ({ workspaceId, status, cursor, limit }) =>
        Effect.gen(function* () {
          if (workspaceId) requireWorkspace(workspaceId);
          const page = paginate(
            yield* listWorkspacePapercuts(workspaceId, status),
            cursor,
            limit,
          );
          return {
            papercuts: page.items.map(serializePapercut),
            nextCursor: page.nextCursor,
            total: page.total,
          };
        }),
    }),
    defineTool({
      name: "workspace_papercut_resolve",
      title: "Resolve Workspace Papercut",
      description:
        "Mark one workspace papercut addressed or dismissed with a durable local resolution note.",
      inputSchema: z
        .object({
          papercutId: z.string().uuid(),
          status: z.enum(["addressed", "dismissed"]),
          resolution: z.string().trim().min(1).max(2_000),
        })
        .strict(),
      outputSchema: z.object({ papercut: workspacePapercutSchema }),
      annotations: annotations(false, false, true, false),
      policy: {
        sideEffects: ["Updates a local papercut's resolution state"],
        cost: "none",
        recommendedPolicy: "allow",
      },
      execute: ({ papercutId, status, resolution }) =>
        Effect.gen(function* () {
          const papercut = yield* resolveWorkspacePapercut(
            papercutId,
            status,
            resolution,
          );
          if (!papercut) throw new Error(`Unknown workspace papercut "${papercutId}"`);
          return { papercut: serializePapercut(papercut) };
        }),
    }),
  ];
}
