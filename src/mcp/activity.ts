import type { Effect as EffectType } from "effect/Effect";
import { randomUUID } from "node:crypto";
import { Entity } from "@micthiesen/mitools/entities";
import type { Docstore } from "@micthiesen/mitools/docstore";
import type { Logger, NamedLogger } from "@micthiesen/mitools/logging";
import { Cause, Clock, Effect, Exit } from "effect";
import type { ExecutorPolicy, McpToolDefinition } from "./tool.js";

export const MCP_ACTIVITY_MAX_CALLS = 2_000;
const PRUNE_SLACK = 100;
const DEFAULT_STRING_LIMIT = 300;
const CLAUDE_STRING_LIMIT = 4_000;
const ARRAY_LIMIT = 20;
const DEPTH_LIMIT = 6;
const ERROR_LIMIT = 2_000;
const SECRET_KEY = /token|secret|password|passwd|authorization|api[_-]?key|cookie/i;

export type McpCallStatus = "running" | "ok" | "error" | "interrupted";

export type McpCallData = {
  callId: string;
  tool: string;
  title: string;
  recommendedPolicy: ExecutorPolicy;
  readOnly: boolean;
  startedAt: number;
  finishedAt: number | null;
  durationMs: number | null;
  status: McpCallStatus;
  error: string | null;
  input: unknown;
  /** Recorded only for Claude session tools, whose results are the activity. */
  output: unknown;
};

export const McpCallEntity = new Entity<McpCallData, ["callId"]>("mcp-call", [
  "callId",
]);

export function isClaudeTool(tool: string): boolean {
  return tool.startsWith("claude_");
}

function truncateText(value: string, limit: number): string {
  return value.length > limit
    ? `${value.slice(0, limit)}… [truncated ${value.length - limit} chars]`
    : value;
}

/** Bound a JSON-like value for storage: cap strings, arrays, depth; hide secrets. */
export function boundValue(value: unknown, stringLimit: number, depth = 0): unknown {
  if (typeof value === "string") return truncateText(value, stringLimit);
  if (value === null || typeof value !== "object") {
    return typeof value === "function" || typeof value === "symbol"
      ? String(value)
      : value;
  }
  if (depth >= DEPTH_LIMIT) return "[nested]";
  if (Array.isArray(value)) {
    const items = value
      .slice(0, ARRAY_LIMIT)
      .map((item) => boundValue(item, stringLimit, depth + 1));
    if (value.length > ARRAY_LIMIT) {
      items.push(`… [${value.length - ARRAY_LIMIT} more items]`);
    }
    return items;
  }
  const result: Record<string, unknown> = {};
  for (const [key, entry] of Object.entries(value)) {
    if (entry === undefined) continue;
    result[key] = SECRET_KEY.test(key)
      ? "[redacted]"
      : boundValue(entry, stringLimit, depth + 1);
  }
  return result;
}

function stringLimitFor(tool: string): number {
  return isClaudeTool(tool) ? CLAUDE_STRING_LIMIT : DEFAULT_STRING_LIMIT;
}

let recordedSincePrune = 0;

const pruneCalls = Effect.fn("McpActivity.prune")(function* () {
  if ((yield* McpCallEntity.count()) <= MCP_ACTIVITY_MAX_CALLS + PRUNE_SLACK) return 0;
  const calls = yield* McpCallEntity.getAll();
  calls.sort((a, b) => b.startedAt - a.startedAt);
  const stale = calls.slice(MCP_ACTIVITY_MAX_CALLS);
  for (const call of stale) yield* McpCallEntity.delete({ callId: call.callId });
  return stale.length;
});

/**
 * Run one MCP tool call and record it. Recording failures are logged and never
 * change the tool's own result.
 */
export function withCallRecording<A, E, R>(
  tool: Pick<McpToolDefinition, "name" | "title" | "policy" | "annotations">,
  input: unknown,
  logger: NamedLogger,
  call: EffectType<A, E, R>,
): EffectType<A, E, R | Docstore | Logger> {
  const limit = stringLimitFor(tool.name);
  const safely = (label: string, effect: EffectType<unknown, unknown, Docstore>) =>
    effect.pipe(
      Effect.catchCause((cause) =>
        logger.warn(`MCP activity ${label} failed for ${tool.name}: ${String(cause)}`),
      ),
    );

  return Effect.gen(function* () {
    const callId = yield* Effect.sync(() => randomUUID());
    const startedAt = yield* Clock.currentTimeMillis;
    yield* safely(
      "start",
      McpCallEntity.upsert({
        callId,
        tool: tool.name,
        title: tool.title,
        recommendedPolicy: tool.policy.recommendedPolicy,
        readOnly: tool.annotations.readOnlyHint,
        startedAt,
        finishedAt: null,
        durationMs: null,
        status: "running",
        error: null,
        input: boundValue(input, limit),
        output: null,
      }),
    );
    const finish = (status: McpCallStatus, error: string | null, output: unknown) =>
      Effect.gen(function* () {
        const finishedAt = yield* Clock.currentTimeMillis;
        yield* safely(
          "finish",
          McpCallEntity.patch(
            { callId },
            {
              finishedAt,
              durationMs: finishedAt - startedAt,
              status,
              error: error === null ? null : truncateText(error, ERROR_LIMIT),
              output:
                output !== null && isClaudeTool(tool.name)
                  ? boundValue(output, limit)
                  : null,
            },
          ),
        );
        recordedSincePrune += 1;
        if (recordedSincePrune >= PRUNE_SLACK) {
          recordedSincePrune = 0;
          yield* safely("prune", pruneCalls());
        }
      });

    return yield* call.pipe(
      Effect.onExit((exit) =>
        Exit.isSuccess(exit)
          ? finish("ok", null, exit.value)
          : Cause.hasInterruptsOnly(exit.cause)
            ? finish("interrupted", "The MCP request was cancelled", null)
            : finish("error", errorMessage(Cause.squash(exit.cause)), null),
      ),
    );
  });
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** Calls left running by a restarted process can never finish. Call at boot. */
export const markInterruptedCalls = Effect.fn("McpActivity.markInterrupted")(
  function* () {
    const running = (yield* McpCallEntity.getAll()).filter(
      (call) => call.status === "running",
    );
    const now = yield* Clock.currentTimeMillis;
    for (const call of running) {
      yield* McpCallEntity.patch(
        { callId: call.callId },
        {
          status: "interrupted",
          error: "Omni restarted before the call finished",
          finishedAt: now,
          durationMs: now - call.startedAt,
        },
      );
    }
    yield* pruneCalls();
    return running.length;
  },
);

export type McpActivityQuery = {
  limit: number;
  tool?: string;
  status?: McpCallStatus;
  before?: number;
  toolPrefix?: string;
};

export type McpToolSummary = {
  tool: string;
  title: string;
  calls: number;
  errors: number;
  lastAt: number;
  avgDurationMs: number | null;
  recommendedPolicy: ExecutorPolicy;
};

const DAY_MS = 24 * 60 * 60 * 1000;

/** Pure view over stored calls for the activity pages. */
export function summarizeCalls(
  stored: readonly McpCallData[],
  query: McpActivityQuery,
  now: number,
) {
  const newestFirst = [...stored].sort((a, b) => b.startedAt - a.startedAt);
  const scoped = query.toolPrefix
    ? newestFirst.filter((call) => call.tool.startsWith(query.toolPrefix!))
    : newestFirst;
  const matching = scoped.filter(
    (call) =>
      (!query.tool || call.tool === query.tool) &&
      (!query.status || call.status === query.status) &&
      (query.before === undefined || call.startedAt < query.before),
  );
  const calls = matching.slice(0, query.limit);
  const nextBefore =
    matching.length > calls.length ? (calls.at(-1)?.startedAt ?? null) : null;

  const recent = scoped.filter((call) => now - call.startedAt < DAY_MS);
  const byTool = new Map<
    string,
    McpToolSummary & { durationTotal: number; timed: number }
  >();
  for (const call of scoped) {
    const entry = byTool.get(call.tool) ?? {
      tool: call.tool,
      title: call.title,
      calls: 0,
      errors: 0,
      lastAt: call.startedAt,
      avgDurationMs: null,
      recommendedPolicy: call.recommendedPolicy,
      durationTotal: 0,
      timed: 0,
    };
    entry.calls += 1;
    if (call.status === "error" || call.status === "interrupted") entry.errors += 1;
    entry.lastAt = Math.max(entry.lastAt, call.startedAt);
    if (call.durationMs !== null) {
      entry.durationTotal += call.durationMs;
      entry.timed += 1;
    }
    byTool.set(call.tool, entry);
  }
  const tools = [...byTool.values()]
    .map(({ durationTotal, timed, ...entry }) => ({
      ...entry,
      avgDurationMs: timed ? Math.round(durationTotal / timed) : null,
    }))
    .sort((a, b) => b.lastAt - a.lastAt);

  return {
    calls,
    nextBefore,
    summary: {
      stored: scoped.length,
      last24h: recent.length,
      errors24h: recent.filter(
        (c) => c.status === "error" || c.status === "interrupted",
      ).length,
      running: scoped.filter((c) => c.status === "running").length,
      approvalCalls24h: recent.filter((c) => c.recommendedPolicy === "require_approval")
        .length,
    },
    tools,
    retention: { maxCalls: MCP_ACTIVITY_MAX_CALLS },
  };
}

export const getMcpActivity = Effect.fn("McpActivity.get")(function* (
  query: McpActivityQuery,
) {
  const stored = yield* McpCallEntity.getAll();
  return summarizeCalls(stored, query, yield* Clock.currentTimeMillis);
});
