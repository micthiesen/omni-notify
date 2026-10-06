import { Logger } from "@micthiesen/mitools/logging";
import { Effect } from "effect";
import { afterAll, describe, expect, it } from "vitest";
import type { DeviceCommand, DeviceLinkService } from "../../device-link/service.js";
import type { ClaudeSessionWatcher } from "../events/claudeSessions.js";
import type { McpRuntime } from "../runtime.js";
import { createMitoolsTestRuntime } from "../../test/mitools.js";
import {
  createClaudeSessionTools,
  scrubHostDetails,
  toItem,
  toSession,
} from "./claude-sessions.js";

const testRuntime = createMitoolsTestRuntime();
afterAll(() => testRuntime.dispose());

const SESSION = "25ffb449-1111-4222-8333-444455556666";

function harness(responses: Partial<Record<DeviceCommand, Record<string, unknown>>>) {
  const calls: { command: DeviceCommand; args: Record<string, unknown> }[] = [];
  const noted: unknown[] = [];
  const deviceLink = {
    status: () =>
      Effect.succeed({
        configured: true as const,
        online: true,
        disabled: false,
        host: "studio",
        lastSeenAt: null,
        pendingJobs: 0,
      }),
    execute: (command: DeviceCommand, args: Record<string, unknown>) => {
      calls.push({ command, args });
      return Effect.succeed(responses[command] ?? {});
    },
  } as unknown as DeviceLinkService;
  const claudeWatcher = {
    noteTurnStarted: (input: unknown) => Effect.sync(() => void noted.push(input)),
  } as unknown as ClaudeSessionWatcher;
  const runtime = {
    logger: Logger.named("ClaudeToolsSpec"),
    deviceLink,
    claudeWatcher,
  } as unknown as McpRuntime;
  const tools = new Map(
    createClaudeSessionTools(runtime).map((tool) => [tool.name, tool]),
  );
  const call = (name: string, input: Record<string, unknown>) =>
    testRuntime.run(
      tools.get(name)!.execute(input) as Effect.Effect<Record<string, unknown>>,
    );
  return { calls, noted, tools, call };
}

const summary = (revision: number, status = "idle") => ({
  id: "25ffb449",
  session_id: SESSION,
  status,
  state: status === "busy" ? "working" : "idle",
  revision,
});

describe("Claude session tools", () => {
  it("keeps one read tool for status, waiting, and results", async () => {
    const { tools } = harness({});
    expect([...tools.keys()]).toEqual([
      "claude_link_status",
      "claude_sessions_list",
      "claude_session_get",
      "claude_session_read",
      "claude_session_start",
      "claude_session_send",
      "claude_session_stop",
    ]);
  });

  it("waits, then reads the result only once the turn finished", async () => {
    const finished = harness({
      wait: { ...summary(6), timed_out: false },
      result: { ...summary(6), result: "Done." },
    });
    const output = await finished.call("claude_session_get", {
      session: "25ffb449",
      afterRevision: 4,
      waitSeconds: 30,
      includeResult: true,
    });
    expect(finished.calls.map((call) => call.command)).toEqual(["wait", "result"]);
    expect(finished.calls[0]!.args).toEqual({
      session: "25ffb449",
      after: 4,
      timeout: 30,
    });
    expect(output).toMatchObject({ timedOut: false, result: "Done." });

    const pending = harness({ wait: { ...summary(5, "busy"), timed_out: true } });
    const waited = await pending.call("claude_session_get", {
      session: "25ffb449",
      waitSeconds: 10,
      includeResult: true,
    });
    expect(pending.calls.map((call) => call.command)).toEqual(["wait"]);
    expect(waited).toMatchObject({ timedOut: true, result: null });

    const status = harness({ status: summary(5) });
    await status.call("claude_session_get", { session: "25ffb449" });
    expect(status.calls.map((call) => call.command)).toEqual(["status"]);
  });

  it("forwards the send key and notes only new turns for events", async () => {
    const sent = harness({ send: { ...summary(5, "busy"), previous_revision: 4 } });
    const output = await sent.call("claude_session_send", {
      session: "25ffb449",
      prompt: "Continue",
      idempotencyKey: "dot:1",
    });
    expect(sent.calls[0]!.args).toMatchObject({ idempotencyKey: "dot:1" });
    expect(output).toMatchObject({ previousRevision: 4, reused: false });
    expect(sent.noted).toEqual([
      { sessionId: SESSION, id: "25ffb449", project: null, revision: 4 },
    ]);

    const reused = harness({
      send: { ...summary(6), previous_revision: 4, reused: true },
    });
    await reused.call("claude_session_send", {
      session: "25ffb449",
      prompt: "Continue",
      idempotencyKey: "dot:1",
    });
    expect(reused.noted).toEqual([]);
  });

  it("reports projects with the link status", async () => {
    const { call } = harness({
      projects: {
        projects: [{ name: "omni-notify", path: "~/Code/omni", exists: true }],
      },
    });
    expect(await call("claude_link_status", {})).toMatchObject({
      online: true,
      projects: [{ name: "omni-notify", exists: true }],
      projectsError: null,
    });
  });
});

describe("Claude session tool normalization", () => {
  it("maps claude-for-dot summaries to the MCP session shape", () => {
    expect(
      toSession({
        id: "a1b2c3",
        session_id: "a1b2c3d4-0000-0000-0000-000000000000",
        kind: "background",
        title: "Fix reconnect",
        cwd: "/Users/michael/Code/omni-notify",
        project: "omni-notify",
        status: "idle",
        state: "done",
        started_at: 1_790_000_000_000,
        revision: 12,
        last_assistant: "Done.",
        managed: true,
      }),
    ).toEqual({
      id: "a1b2c3",
      sessionId: "a1b2c3d4-0000-0000-0000-000000000000",
      kind: "background",
      title: "Fix reconnect",
      cwd: "/Users/michael/Code/omni-notify",
      project: "omni-notify",
      status: "idle",
      state: "done",
      startedAt: new Date(1_790_000_000_000).toISOString(),
      revision: 12,
      lastAssistant: "Done.",
    });
  });

  it("tolerates missing fields from history-only sessions", () => {
    expect(toSession({ session_id: "abc", status: "stopped" })).toMatchObject({
      id: null,
      project: null,
      startedAt: null,
      revision: 0,
    });
  });

  it("bounds transcript text and tool inputs", () => {
    const item = toItem({
      index: 3,
      kind: "assistant",
      timestamp: "2026-10-04T00:00:00Z",
      text: "x".repeat(9_000),
    });
    expect(item.text).toHaveLength(8_000);
    expect(item.truncated).toBe(true);
    const tool = toItem({
      index: 4,
      kind: "tool_use",
      tool: "Bash",
      input: { command: "y".repeat(2_000) },
    });
    expect(tool).toMatchObject({ tool: "Bash", text: null, truncated: false });
    expect(tool.input).toHaveLength(1_000);
  });

  it("removes the host name and home directory from results", () => {
    expect(
      scrubHostDetails(
        {
          cwd: "/Users/michael/Code/omni-notify",
          items: [{ text: "Ran on MaxBook in /Users/michael/.dotfiles" }],
          revision: 3,
        },
        "MaxBook",
      ),
    ).toEqual({
      cwd: "~/Code/omni-notify",
      items: [{ text: "Ran on the host in ~/.dotfiles" }],
      revision: 3,
    });
    expect(scrubHostDetails("maxbookish", "MaxBook")).toBe("maxbookish");
  });
});
