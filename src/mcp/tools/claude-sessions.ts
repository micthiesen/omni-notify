import { Clock, Duration, Effect } from "effect";
import { z } from "zod";
import {
  type DeviceCommand,
  DeviceLinkError,
  type DeviceLinkService,
} from "../../device-link/service.js";
import type { McpRuntime } from "../runtime.js";
import {
  annotations,
  defineTool,
  emptyInputSchema,
  type McpToolDefinition,
  truncate,
} from "../tool.js";

const ITEM_TEXT_LIMIT = 8_000;
const RESULT_TEXT_LIMIT = 20_000;
const WAIT_MAX_SECONDS = 45;

const sessionId = z
  .string()
  .regex(/^[0-9A-Za-z-]{4,64}$/)
  .describe(
    "Short id from claude_sessions_list, a full session id, or a unique prefix",
  );
const projectName = z
  .string()
  .regex(/^[A-Za-z0-9][A-Za-z0-9._-]*$/)
  .max(100)
  .describe("Project name from claude_projects_list");
const prompt = z.string().min(1).max(100_000);

const sessionSchema = z.object({
  id: z.string().nullable(),
  sessionId: z.string(),
  kind: z.string().nullable(),
  title: z.string().nullable(),
  cwd: z.string().nullable(),
  project: z.string().nullable(),
  status: z.string(),
  state: z.string().nullable(),
  startedAt: z.string().nullable(),
  revision: z.number().int().nonnegative(),
  lastAssistant: z.string().nullable(),
});
type Session = z.infer<typeof sessionSchema>;

const itemSchema = z.object({
  index: z.number().int().nonnegative(),
  kind: z.string(),
  timestamp: z.string().nullable(),
  text: z.string().nullable(),
  truncated: z.boolean(),
  tool: z.string().nullable(),
  input: z.string().nullable(),
  isError: z.boolean().nullable(),
});

type Raw = Record<string, unknown>;

const str = (value: unknown): string | null =>
  typeof value === "string" ? value : null;
const int = (value: unknown): number =>
  typeof value === "number" && Number.isInteger(value) && value >= 0 ? value : 0;
const record = (value: unknown): Raw =>
  typeof value === "object" && value !== null && !Array.isArray(value)
    ? (value as Raw)
    : {};
const records = (value: unknown): Raw[] =>
  Array.isArray(value) ? value.map(record) : [];

/** Normalize one claude-for-dot session summary into the MCP shape. */
export function toSession(raw: Raw): Session {
  const started = raw.started_at;
  return {
    id: str(raw.id),
    sessionId: str(raw.session_id) ?? "",
    kind: str(raw.kind),
    title: str(raw.title),
    cwd: str(raw.cwd),
    project: str(raw.project),
    status: str(raw.status) ?? "unknown",
    state: str(raw.state),
    startedAt:
      typeof started === "number" && Number.isFinite(started)
        ? new Date(started).toISOString()
        : null,
    revision: int(raw.revision),
    lastAssistant: str(raw.last_assistant),
  };
}

export function toItem(raw: Raw): z.infer<typeof itemSchema> {
  const text = str(raw.text);
  const bounded = text === null ? null : truncate(text, ITEM_TEXT_LIMIT);
  const input =
    raw.input === undefined ? null : truncate(JSON.stringify(raw.input), 1_000).text;
  return {
    index: int(raw.index),
    kind: str(raw.kind) ?? "unknown",
    timestamp: str(raw.timestamp),
    text: bounded?.text ?? null,
    truncated: bounded?.truncated ?? false,
    tool: str(raw.tool),
    input,
    isError: typeof raw.is_error === "boolean" ? raw.is_error : null,
  };
}

const HOME_DIRECTORY = /\/Users\/[^/\s"'`]+/g;

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/**
 * MCP results describe a generic Claude Code host. They never name the machine
 * or expose its home directory; Omni's own UI may still show both.
 */
export function scrubHostDetails(value: unknown, host: string | null): unknown {
  const hostName =
    host && host.length >= 3 ? new RegExp(`\\b${escapeRegExp(host)}\\b`, "gi") : null;
  const scrub = (item: unknown): unknown => {
    if (typeof item === "string") {
      const homeless = item.replace(HOME_DIRECTORY, "~");
      return hostName ? homeless.replace(hostName, "the host") : homeless;
    }
    if (Array.isArray(item)) return item.map(scrub);
    if (typeof item === "object" && item !== null) {
      return Object.fromEntries(
        Object.entries(item).map(([key, entry]) => [key, scrub(entry)]),
      );
    }
    return item;
  };
  return scrub(value);
}

const GENERIC_DETAILS: Record<string, string> = {
  disabled:
    "Session control is disabled on the Claude Code host (kill switch); nothing ran",
};

function publicError(error: DeviceLinkError, host: string | null): DeviceLinkError {
  return new DeviceLinkError({
    code: error.code,
    retryable: error.retryable,
    detail: GENERIC_DETAILS[error.code] ?? String(scrubHostDetails(error.detail, host)),
  });
}

function requireLink(runtime: McpRuntime): DeviceLinkService {
  if (!runtime.deviceLink)
    throw new Error("The Claude Code host link is not configured");
  return runtime.deviceLink;
}

function run(
  runtime: McpRuntime,
  command: DeviceCommand,
  args: Raw,
  timeout: Duration.Duration,
) {
  const logger = runtime.logger.extend("MCP:ClaudeSessions");
  const target = str(args.session) ?? str(args.project) ?? "-";
  return Effect.gen(function* () {
    const link = yield* Effect.try(() => requireLink(runtime));
    const started = yield* Clock.currentTimeMillis;
    const elapsed = Clock.currentTimeMillis.pipe(Effect.map((now) => now - started));
    const { host } = yield* link.status();
    return yield* link.execute(command, args, timeout).pipe(
      Effect.map((data) => scrubHostDetails(data, host) as Raw),
      Effect.mapError((error) => publicError(error, host)),
      Effect.tap(() =>
        elapsed.pipe(
          Effect.flatMap((ms) =>
            logger.info(`claude ${command} ${target} ok in ${ms}ms`),
          ),
        ),
      ),
      Effect.tapError((error) =>
        elapsed.pipe(
          Effect.flatMap((ms) =>
            logger.warn(
              `claude ${command} ${target} failed (${error.code}) in ${ms}ms`,
            ),
          ),
        ),
      ),
    );
  });
}

/** Lets turn_finished fire for a turn that ends before the watcher's next poll. */
function noteTurnStarted(runtime: McpRuntime, session: Session, revision: number) {
  if (!runtime.claudeWatcher || !session.sessionId) return Effect.void;
  return runtime.claudeWatcher
    .noteTurnStarted({
      sessionId: session.sessionId,
      id: session.id,
      project: session.project,
      revision,
    })
    .pipe(Effect.ignoreCause);
}

const QUICK = Duration.seconds(60);
const LAUNCH = Duration.seconds(150);

const readPolicy = (effect: string) => ({
  sideEffects: [effect],
  cost: "none",
  recommendedPolicy: "allow" as const,
});

export function createClaudeSessionTools(runtime: McpRuntime): McpToolDefinition[] {
  return [
    defineTool({
      name: "claude_link_status",
      title: "Get Claude Code Host Status",
      description:
        "Report whether the Claude Code host is connected to Omni for session control, whether its kill switch is engaged, when it last checked in, and the projects where new sessions may start (the same list its Remote Control servers use; null while the host is unreachable). Start sessions by project name.",
      inputSchema: emptyInputSchema,
      outputSchema: z.object({
        configured: z.boolean(),
        online: z.boolean(),
        disabled: z.boolean(),
        lastSeenAt: z.string().nullable(),
        pendingJobs: z.number().int().nonnegative(),
        projects: z
          .array(z.object({ name: z.string(), path: z.string(), exists: z.boolean() }))
          .nullable(),
        projectsError: z.string().nullable(),
      }),
      annotations: annotations(true, false, true, false),
      policy: readPolicy(
        "Reads Omni's link state and the host's project configuration",
      ),
      execute: () =>
        Effect.gen(function* () {
          if (!runtime.deviceLink) {
            return {
              configured: false,
              online: false,
              disabled: false,
              lastSeenAt: null,
              pendingJobs: 0,
              projects: null,
              projectsError: "The Claude Code host link is not configured",
            };
          }
          const { host: _host, ...status } = yield* runtime.deviceLink.status();
          const projects =
            status.online && !status.disabled
              ? yield* run(runtime, "projects", {}, QUICK).pipe(Effect.result)
              : undefined;
          return {
            ...status,
            projects:
              projects?._tag === "Success"
                ? records(projects.success.projects).map((project) => ({
                    name: str(project.name) ?? "",
                    path: str(project.path) ?? "",
                    exists: project.exists === true,
                  }))
                : null,
            projectsError:
              projects === undefined
                ? status.disabled
                  ? "Session control is disabled on the Claude Code host"
                  : "The Claude Code host is offline"
                : projects._tag === "Failure"
                  ? projects.failure.message
                  : null,
          };
        }),
    }),
    defineTool({
      name: "claude_sessions_list",
      title: "List Claude Code Sessions",
      description:
        "List Claude Code sessions on the Claude Code host, newest first: background, interactive terminal, and Remote Control sessions in any directory. Each session reports its configured project when its directory belongs to one. Filter by project name to narrow the list.",
      inputSchema: z
        .object({
          project: projectName.optional(),
          includeStopped: z.boolean().default(false),
          limit: z.number().int().min(1).max(100).default(25),
        })
        .strict(),
      outputSchema: z.object({ sessions: z.array(sessionSchema) }),
      annotations: annotations(true, false, true, false),
      policy: readPolicy("Reads session metadata and transcripts on the host"),
      execute: (input) =>
        run(
          runtime,
          "list",
          { project: input.project, all: input.includeStopped, limit: input.limit },
          QUICK,
        ).pipe(
          Effect.map((data) => ({ sessions: records(data.sessions).map(toSession) })),
        ),
    }),
    defineTool({
      name: "claude_session_get",
      title: "Get Claude Code Session",
      description: `Read one Claude Code session's status, transcript revision, and last assistant text. To wait for a turn to finish, pass waitSeconds (up to ${WAIT_MAX_SECONDS}) with afterRevision from a start or send result so the wait cannot return before the new turn; when timedOut is true, call again. includeResult adds all assistant text since the last user input once the turn has finished. Subscribing to the claude.session.turn_finished MCP event avoids repeated waits where the client supports MCP Events.`,
      inputSchema: z
        .object({
          session: sessionId,
          afterRevision: z.number().int().min(0).optional(),
          waitSeconds: z.number().int().min(0).max(WAIT_MAX_SECONDS).default(0),
          includeResult: z.boolean().default(false),
        })
        .strict(),
      outputSchema: z.object({
        session: sessionSchema,
        timedOut: z.boolean(),
        result: z.string().nullable(),
        truncated: z.boolean(),
      }),
      annotations: annotations(true, false, true, false),
      policy: readPolicy("Reads session status and transcripts on the host"),
      execute: (input) =>
        Effect.gen(function* () {
          const waited =
            input.waitSeconds > 0
              ? yield* run(
                  runtime,
                  "wait",
                  {
                    session: input.session,
                    after: input.afterRevision,
                    timeout: input.waitSeconds,
                  },
                  Duration.seconds(input.waitSeconds + 20),
                )
              : undefined;
          const timedOut = waited?.timed_out === true;
          if (!input.includeResult || timedOut) {
            const data =
              waited ??
              (yield* run(runtime, "status", { session: input.session }, QUICK));
            return {
              session: toSession(data),
              timedOut,
              result: null,
              truncated: false,
            };
          }
          const data = yield* run(runtime, "result", { session: input.session }, QUICK);
          const text = str(data.result);
          const bounded = text === null ? null : truncate(text, RESULT_TEXT_LIMIT);
          return {
            session: toSession(data),
            timedOut: false,
            result: bounded?.text ?? null,
            truncated: bounded?.truncated ?? false,
          };
        }),
    }),
    defineTool({
      name: "claude_session_read",
      title: "Read Claude Code Transcript",
      description:
        "Page through a Claude Code session's transcript: user and assistant text, tool calls, and abbreviated tool results. Without a cursor it returns the latest items; pass nextCursor to continue forward.",
      inputSchema: z
        .object({
          session: sessionId,
          limit: z.number().int().min(1).max(100).default(20),
          cursor: z.number().int().min(0).optional(),
        })
        .strict(),
      outputSchema: z.object({
        sessionId: z.string(),
        items: z.array(itemSchema),
        revision: z.number().int().nonnegative(),
        nextCursor: z.number().int().nullable(),
        hasMore: z.boolean(),
      }),
      annotations: annotations(true, false, true, false),
      policy: readPolicy("Reads a session transcript on the host"),
      execute: (input) =>
        run(
          runtime,
          "read",
          { session: input.session, limit: input.limit, cursor: input.cursor },
          QUICK,
        ).pipe(
          Effect.map((data) => ({
            sessionId: str(data.session_id) ?? "",
            items: records(data.items).map(toItem),
            revision: int(data.revision),
            nextCursor: typeof data.next_cursor === "number" ? data.next_cursor : null,
            hasMore: data.has_more === true,
          })),
        ),
    }),
    // Sessions intentionally run with full access (bypass permissions), like
    // claude-for-dot. Approval happens before start and send through the
    // require_approval policy, not inside the session, so a background session
    // never stalls on a permission prompt. Do not add a permission mode here.
    defineTool({
      name: "claude_session_start",
      title: "Start Claude Code Session",
      description:
        "Start a new background Claude Code session on the Claude Code host in a configured project (see claude_link_status). The session runs with full access (bypass permissions) and can edit, run commands, commit, and push in that project. Reusing an idempotencyKey for the same project returns the existing session instead of starting another.",
      inputSchema: z
        .object({
          project: projectName,
          prompt,
          idempotencyKey: z
            .string()
            .regex(/^[A-Za-z0-9._:-]{1,128}$/)
            .describe("Caller-chosen key that makes retries safe"),
          title: z.string().trim().min(1).max(200).optional(),
          model: z
            .string()
            .regex(/^[A-Za-z0-9._[\]-]{1,64}$/)
            .optional()
            .describe("Claude Code model alias or id, such as sonnet or opus"),
          effort: z.enum(["low", "medium", "high", "xhigh", "max"]).optional(),
        })
        .strict(),
      outputSchema: z.object({ session: sessionSchema, reused: z.boolean() }),
      annotations: annotations(false, true, true, true),
      policy: {
        sideEffects: [
          "Starts a Claude Code process on the host with full access to the project",
          "The agent may edit files, run commands, commit, push, and use the network",
        ],
        cost: "Consumes Claude subscription usage",
        recommendedPolicy: "require_approval",
      },
      execute: (input) =>
        run(
          runtime,
          "start",
          {
            project: input.project,
            prompt: input.prompt,
            idempotencyKey: input.idempotencyKey,
            title: input.title,
            model: input.model,
            effort: input.effort,
          },
          LAUNCH,
        ).pipe(
          Effect.map((data) => ({
            session: toSession(data),
            reused: data.reused === true,
          })),
          Effect.tap(({ session, reused }) =>
            reused ? Effect.void : noteTurnStarted(runtime, session, session.revision),
          ),
        ),
    }),
    defineTool({
      name: "claude_session_send",
      title: "Send Input to Claude Code Session",
      description:
        "Continue an idle background Claude Code session with new user input. Busy sessions are refused unless interrupt is true, which ends the current turn first. Interactive terminal sessions cannot receive input here. Pass an idempotencyKey so a retry returns the earlier send (reused) instead of sending twice; a retry whose earlier attempt never finished fails with send_in_doubt. Without a key, read the session before sending again after an unknown result.",
      inputSchema: z
        .object({
          session: sessionId,
          prompt,
          interrupt: z.boolean().default(false),
          idempotencyKey: z
            .string()
            .regex(/^[A-Za-z0-9._:-]{1,128}$/)
            .optional()
            .describe("Caller-chosen key that makes retries safe"),
        })
        .strict(),
      outputSchema: z.object({
        session: sessionSchema,
        previousRevision: z.number().int().nonnegative(),
        reused: z.boolean(),
        warning: z.string().nullable(),
      }),
      annotations: annotations(false, true, false, true),
      policy: {
        sideEffects: [
          "Resumes a Claude Code session on the host with full access to its directory",
          "The agent may edit files, run commands, commit, push, and use the network",
        ],
        cost: "Consumes Claude subscription usage",
        recommendedPolicy: "require_approval",
      },
      execute: (input) =>
        run(
          runtime,
          "send",
          {
            session: input.session,
            prompt: input.prompt,
            interrupt: input.interrupt,
            idempotencyKey: input.idempotencyKey,
          },
          LAUNCH,
        ).pipe(
          Effect.map((data) => ({
            session: toSession(data),
            previousRevision: int(data.previous_revision),
            reused: data.reused === true,
            warning: str(data.warning),
          })),
          Effect.tap(({ session, previousRevision, reused }) =>
            reused ? Effect.void : noteTurnStarted(runtime, session, previousRevision),
          ),
        ),
    }),
    defineTool({
      name: "claude_session_stop",
      title: "Stop Claude Code Session",
      description:
        "Stop a background Claude Code session on the Claude Code host. The conversation is kept and can be continued later with claude_session_send.",
      inputSchema: z.object({ session: sessionId }).strict(),
      outputSchema: z.object({ id: z.string().nullable(), sessionId: z.string() }),
      annotations: annotations(false, false, true, false),
      policy: {
        sideEffects: ["Ends the session's running process and any turn in progress"],
        cost: "none",
        recommendedPolicy: "require_approval",
      },
      execute: (input) =>
        run(runtime, "stop", { session: input.session }, QUICK).pipe(
          Effect.map((data) => ({
            id: str(data.id),
            sessionId: str(data.session_id) ?? "",
          })),
        ),
    }),
  ];
}
