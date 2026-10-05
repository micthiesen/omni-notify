import type { EffectRunner } from "@micthiesen/mitools/boundary";
import type { Context, Hono } from "hono";
import { Duration, Effect } from "effect";
import { DeviceLinkError, type DeviceLinkService } from "../device-link/service.js";
import { effectHandler } from "../effect/http.js";
import type { AppServices } from "../effect/appRuntime.js";
import { getMcpActivity, type McpCallStatus } from "./activity.js";
import { toItem, toSession } from "./tools/claude-sessions.js";

const STATUSES = new Set<McpCallStatus>(["running", "ok", "error", "interrupted"]);
const SESSION = /^[0-9A-Za-z-]{4,64}$/;
const LIVE_TIMEOUT = Duration.seconds(30);

function boundedInt(value: string | undefined, fallback: number, max: number): number {
  const parsed = Number(value);
  return Number.isInteger(parsed) && parsed > 0 ? Math.min(parsed, max) : fallback;
}

const records = (value: unknown): Record<string, unknown>[] =>
  Array.isArray(value)
    ? value.filter(
        (item): item is Record<string, unknown> =>
          typeof item === "object" && item !== null,
      )
    : [];

function linkFailure(c: Context, error: DeviceLinkError): Response {
  const unavailable = ["offline", "disabled", "not_picked_up", "not_configured"];
  return c.json(
    { error: error.detail, code: error.code },
    unavailable.includes(error.code) ? 503 : 502,
  );
}

/** Read-only observability routes for MCP calls and the Mac's Claude sessions. */
export function registerMcpActivityRoutes(
  runner: EffectRunner<AppServices>,
  app: Hono,
  deviceLink: DeviceLinkService | undefined,
): void {
  const live = (
    c: Context,
    command: "list" | "read" | "projects",
    args: Record<string, unknown>,
    toBody: (data: Record<string, unknown>) => Record<string, unknown>,
  ) =>
    deviceLink
      ? deviceLink.execute(command, args, LIVE_TIMEOUT).pipe(
          Effect.map((data) => c.json(toBody(data))),
          Effect.catch((error: DeviceLinkError) =>
            Effect.succeed(linkFailure(c, error)),
          ),
        )
      : Effect.succeed(
          linkFailure(
            c,
            new DeviceLinkError({
              code: "not_configured",
              detail: "The Mac device link is not configured",
              retryable: false,
            }),
          ),
        );

  app.get(
    "/api/mcp/activity",
    effectHandler(runner, (c) => {
      const status = c.req.query("status") as McpCallStatus | undefined;
      const before = Number(c.req.query("before"));
      return getMcpActivity({
        limit: boundedInt(c.req.query("limit"), 100, 200),
        tool: c.req.query("tool") || undefined,
        status: status && STATUSES.has(status) ? status : undefined,
        before: Number.isFinite(before) && before > 0 ? before : undefined,
      }).pipe(Effect.map((activity) => c.json(activity)));
    }),
  );

  app.get(
    "/api/claude/activity",
    effectHandler(runner, (c) =>
      Effect.gen(function* () {
        const activity = yield* getMcpActivity({
          limit: boundedInt(c.req.query("limit"), 200, 500),
          toolPrefix: "claude_",
        });
        const link = deviceLink
          ? yield* deviceLink.status()
          : {
              configured: false,
              online: false,
              disabled: false,
              host: null,
              lastSeenAt: null,
              pendingJobs: 0,
            };
        return c.json({ link, actions: activity.calls, retention: activity.retention });
      }),
    ),
  );

  app.get(
    "/api/claude/sessions",
    effectHandler(runner, (c) =>
      live(
        c,
        "list",
        {
          all: c.req.query("includeStopped") === "true",
          limit: boundedInt(c.req.query("limit"), 25, 100),
        },
        (data) => ({ sessions: records(data.sessions).map(toSession) }),
      ),
    ),
  );

  app.get(
    "/api/claude/sessions/:session/transcript",
    effectHandler(runner, (c) => {
      const session = c.req.param("session") ?? "";
      if (!SESSION.test(session)) {
        return Effect.succeed(
          c.json({ error: "Invalid session id", code: "bad_request" }, 400),
        );
      }
      const cursor = Number(c.req.query("cursor"));
      return live(
        c,
        "read",
        {
          session,
          limit: boundedInt(c.req.query("limit"), 40, 100),
          cursor: Number.isInteger(cursor) && cursor >= 0 ? cursor : undefined,
        },
        (data) => ({
          sessionId: typeof data.session_id === "string" ? data.session_id : session,
          items: records(data.items).map(toItem),
          revision: typeof data.revision === "number" ? data.revision : 0,
          nextCursor: typeof data.next_cursor === "number" ? data.next_cursor : null,
          hasMore: data.has_more === true,
        }),
      );
    }),
  );

  app.get(
    "/api/claude/projects",
    effectHandler(runner, (c) =>
      live(c, "projects", {}, (data) => ({
        projects: records(data.projects).map((project) => ({
          name: String(project.name ?? ""),
          path: String(project.path ?? ""),
          exists: project.exists === true,
        })),
      })),
    ),
  );
}
