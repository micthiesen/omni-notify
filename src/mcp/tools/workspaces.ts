import { z } from "zod";
import { Effect, Exit, Schema } from "effect";
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
  truncate,
} from "../tool.js";

const nullableString = z.string().nullable();

const pageSchema = {
  nextCursor: z.number().int().nonnegative().nullable(),
  total: z.number().int().nonnegative(),
};

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

export function createWorkspaceTools(runtime: McpRuntime): McpToolDefinition[] {
  return [
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
