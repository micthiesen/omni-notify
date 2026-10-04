import { describe, expect, it } from "vitest";
import { toItem, toSession } from "./claude-sessions.js";

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
});
