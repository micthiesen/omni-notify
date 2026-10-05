import { describe, expect, it } from "vitest";
import type { McpCall } from "../api";
import {
  explainClaudeError,
  groupClaudeActions,
  summarizeCompactAction,
} from "./claudeActivity";

const FULL_ID = "25ffb449-1111-4222-8333-444455556666";

function call(overrides: Partial<McpCall> & Pick<McpCall, "callId" | "tool">): McpCall {
  return {
    title: overrides.tool,
    recommendedPolicy: "allow",
    readOnly: false,
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

const session = (status: string, revision: number) => ({
  id: "25ffb449",
  sessionId: FULL_ID,
  kind: "background",
  title: "Fix the build",
  cwd: "/Users/michael/Code/omni-notify",
  project: "omni-notify",
  status,
  state: null,
  startedAt: null,
  revision,
  lastAssistant: null,
});

describe("groupClaudeActions", () => {
  it("joins short-id calls without output to the full session group", () => {
    const actions = [
      call({
        callId: "c4",
        tool: "claude_session_send",
        startedAt: 40,
        status: "error",
        error: "offline",
        input: { session: "25ffb449", prompt: "again" },
      }),
      call({
        callId: "c3",
        tool: "claude_link_status",
        startedAt: 30,
        output: { online: true, disabled: false, host: "mbp" },
      }),
      call({
        callId: "c2",
        tool: "claude_session_wait",
        startedAt: 20,
        input: { session: "25ff" },
        output: { session: session("idle", 3), timedOut: false },
      }),
      call({
        callId: "c1",
        tool: "claude_session_start",
        startedAt: 10,
        input: { project: "omni-notify", prompt: "Fix it" },
        output: { session: session("busy", 1), reused: false },
      }),
    ];

    const { sessions, other } = groupClaudeActions(actions);

    expect(other.map((c) => c.callId)).toEqual(["c3"]);
    expect(sessions).toHaveLength(1);
    const [group] = sessions;
    expect(group.sessionId).toBe(FULL_ID);
    expect(group.actions.map((c) => c.callId)).toEqual(["c1", "c2", "c4"]);
    expect(group.status).toBe("idle");
    expect(group.project).toBe("omni-notify");
    expect(group.latestAt).toBe(40);
    expect(group.latestFailed).toBe(true);
  });

  it("keeps a failed start without a session in its own group, newest first", () => {
    const { sessions } = groupClaudeActions([
      call({
        callId: "late",
        tool: "claude_session_start",
        startedAt: 50,
        status: "error",
        error: "disabled",
        input: { project: "dotfiles", prompt: "x", title: "Tidy" },
      }),
      call({
        callId: "early",
        tool: "claude_session_stop",
        startedAt: 5,
        input: { session: "abc" },
        output: { id: "abc", sessionId: "abc-full" },
      }),
    ]);
    expect(sessions.map((g) => g.key)).toEqual(["call:late", "abc-full"]);
    expect(sessions[0].title).toBe("Tidy");
    expect(sessions[0].project).toBe("dotfiles");
  });
});

describe("explainClaudeError", () => {
  it("recognizes link failure codes in messages", () => {
    expect(explainClaudeError("Job outcome_unknown after timeout").code).toBe(
      "outcome_unknown",
    );
    expect(explainClaudeError("The Mac link is offline").code).toBe("offline");
    expect(explainClaudeError("job was not picked up").code).toBe("not_picked_up");
    expect(explainClaudeError("boom")).toEqual({ code: null, hint: null });
  });

  it("prefers the trailing code the device link appends", () => {
    expect(
      explainClaudeError("The Mac is offline (last seen 5m ago) (offline)").code,
    ).toBe("offline");
    expect(explainClaudeError("Mac did a thing; offline? (outcome_unknown)").code).toBe(
      "outcome_unknown",
    );
    expect(explainClaudeError("No such session (not_found)")).toEqual({
      code: "not_found",
      hint: null,
    });
    expect(explainClaudeError("disabled").code).toBe("disabled");
  });
});

describe("summarizeCompactAction", () => {
  it("summarizes listings and reads", () => {
    expect(
      summarizeCompactAction(
        call({
          callId: "l",
          tool: "claude_sessions_list",
          input: { project: "omni-notify" },
          output: { sessions: [{}, {}] },
        }),
      ),
    ).toBe("2 sessions in omni-notify");
    expect(
      summarizeCompactAction(
        call({
          callId: "r",
          tool: "claude_session_read",
          output: { items: [{}], hasMore: true },
        }),
      ),
    ).toBe("1 item, more available");
    expect(
      summarizeCompactAction(
        call({
          callId: "s",
          tool: "claude_link_status",
          output: { online: false, disabled: true, host: "mbp" },
        }),
      ),
    ).toBe("disabled · mbp");
  });
});
