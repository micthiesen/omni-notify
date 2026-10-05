import { Logger } from "@micthiesen/mitools/logging";
import { Effect } from "effect";
import { afterEach, describe, expect, it } from "vitest";
import { runTest } from "../live-check/testRuntime.js";
import {
  boundValue,
  getMcpActivity,
  markInterruptedCalls,
  McpCallEntity,
  type McpCallData,
  summarizeCalls,
  withCallRecording,
} from "./activity.js";
import { annotations } from "./tool.js";

const logger = Logger.named("McpActivitySpec");

const tool = (name: string) => ({
  name,
  title: `Title ${name}`,
  annotations: annotations(false, false, false, false),
  policy: {
    sideEffects: [],
    cost: "none",
    recommendedPolicy: "require_approval" as const,
  },
});

function call(overrides: Partial<McpCallData>): McpCallData {
  return {
    callId: overrides.callId ?? "c",
    tool: "email_search",
    title: "Search Email",
    recommendedPolicy: "allow",
    readOnly: true,
    startedAt: 0,
    finishedAt: null,
    durationMs: null,
    status: "ok",
    error: null,
    input: {},
    output: null,
    ...overrides,
  };
}

afterEach(() => runTest(McpCallEntity.deleteAll()));

describe("MCP activity bounding", () => {
  it("caps strings and arrays and hides secret-looking keys", () => {
    const bounded = boundValue(
      {
        prompt: "x".repeat(10),
        items: Array.from({ length: 22 }, (_, index) => index),
        apiKey: "secret",
        nested: { authorization: "Bearer abc", keep: true },
        skipped: undefined,
      },
      4,
    );
    expect(bounded).toEqual({
      prompt: "xxxx… [truncated 6 chars]",
      items: [...Array.from({ length: 20 }, (_, index) => index), "… [2 more items]"],
      apiKey: "[redacted]",
      nested: { authorization: "[redacted]", keep: true },
    });
  });
});

describe("MCP activity recording", () => {
  it("records successful Claude calls with their output and others without", async () => {
    await runTest(
      withCallRecording(
        tool("claude_session_start"),
        { project: "omni-notify", prompt: "Do it", idempotencyKey: "k" },
        logger,
        Effect.succeed({ session: { sessionId: "abc" }, reused: false }),
      ),
    );
    await runTest(
      withCallRecording(
        tool("email_search"),
        { query: "invoice" },
        logger,
        Effect.succeed({ messages: ["private"] }),
      ),
    );
    const calls = await runTest(McpCallEntity.getAll());
    const claude = calls.find((c) => c.tool === "claude_session_start");
    const email = calls.find((c) => c.tool === "email_search");
    expect(claude).toMatchObject({
      status: "ok",
      recommendedPolicy: "require_approval",
      input: { project: "omni-notify", prompt: "Do it", idempotencyKey: "k" },
      output: { session: { sessionId: "abc" }, reused: false },
    });
    expect(claude?.durationMs).toBeGreaterThanOrEqual(0);
    expect(email).toMatchObject({
      status: "ok",
      input: { query: "invoice" },
      output: null,
    });
  });

  it("records failures without changing them", async () => {
    const error = await runTest(
      withCallRecording(
        tool("claude_session_send"),
        { session: "abcd" },
        logger,
        Effect.fail(new Error("The Mac is offline (offline)")),
      ).pipe(Effect.flip),
    );
    expect(error.message).toBe("The Mac is offline (offline)");
    const [recorded] = await runTest(McpCallEntity.getAll());
    expect(recorded).toMatchObject({
      status: "error",
      error: "The Mac is offline (offline)",
    });
  });

  it("marks calls left running by a restart as interrupted", async () => {
    await runTest(McpCallEntity.upsert(call({ callId: "left", status: "running" })));
    expect(await runTest(markInterruptedCalls())).toBe(1);
    const activity = await runTest(getMcpActivity({ limit: 10 }));
    expect(activity.calls[0]).toMatchObject({ callId: "left", status: "interrupted" });
  });
});

describe("MCP activity summary", () => {
  const now = 10 * 24 * 60 * 60 * 1000;
  const stored = [
    call({ callId: "a", startedAt: now - 1_000, durationMs: 100 }),
    call({ callId: "b", startedAt: now - 2_000, status: "error", durationMs: 300 }),
    call({
      callId: "c",
      tool: "claude_session_start",
      title: "Start",
      recommendedPolicy: "require_approval",
      startedAt: now - 3_000,
      status: "running",
    }),
    call({ callId: "d", startedAt: now - 3 * 24 * 60 * 60 * 1000, durationMs: 200 }),
  ];

  it("pages newest first and filters by tool, status, and prefix", () => {
    const page = summarizeCalls(stored, { limit: 2 }, now);
    expect(page.calls.map((c) => c.callId)).toEqual(["a", "b"]);
    expect(page.nextBefore).toBe(now - 2_000);
    const next = summarizeCalls(stored, { limit: 2, before: page.nextBefore! }, now);
    expect(next.calls.map((c) => c.callId)).toEqual(["c", "d"]);
    expect(next.nextBefore).toBeNull();
    expect(
      summarizeCalls(stored, { limit: 10, status: "error" }, now).calls.map(
        (c) => c.callId,
      ),
    ).toEqual(["b"]);
    expect(
      summarizeCalls(stored, { limit: 10, toolPrefix: "claude_" }, now).calls.map(
        (c) => c.callId,
      ),
    ).toEqual(["c"]);
  });

  it("aggregates the last day and per-tool statistics", () => {
    const { summary, tools } = summarizeCalls(stored, { limit: 10 }, now);
    expect(summary).toEqual({
      stored: 4,
      last24h: 3,
      errors24h: 1,
      running: 1,
      approvalCalls24h: 1,
    });
    expect(tools).toEqual([
      expect.objectContaining({
        tool: "email_search",
        calls: 3,
        errors: 1,
        avgDurationMs: 200,
      }),
      expect.objectContaining({
        tool: "claude_session_start",
        calls: 1,
        avgDurationMs: null,
      }),
    ]);
  });
});
