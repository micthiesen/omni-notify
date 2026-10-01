import { Effect, Result } from "effect";
import { afterAll, describe, expect, it } from "vitest";
import { createMitoolsTestRuntime } from "../../test/mitools.js";
import type { McpRuntime } from "../runtime.js";
import { createRemindersTools } from "./reminders.js";

const tools = createRemindersTools({} as McpRuntime);
const runtime = createMitoolsTestRuntime();
afterAll(() => runtime.dispose());
const tool = (name: string) => tools.find((item) => item.name === name)!;

describe("server Reminders MCP boundary", () => {
  it("keeps private reads disabled when the service is absent", async () => {
    const result = await runtime.run(
      tool("list_reminders").execute({}).pipe(Effect.result),
    );
    expect(Result.isFailure(result) && result.failure.message).toBe(
      "Reminders: disabled",
    );
  });

  it("rejects recurrence and missing mutation identity before calling a service", async () => {
    for (const input of [
      { id: "Reminder/test", changeTag: "tag", patch: { title: "Changed" } },
      {
        id: "Reminder/test",
        changeTag: "tag",
        idempotencyKey: "fixture-key-123456",
        patch: { recurrence: "daily" },
      },
    ]) {
      const result = await runtime.run(
        tool("update_reminder").execute(input).pipe(Effect.result),
      );
      expect(Result.isFailure(result) && result.failure.phase).toBe("input");
    }
  });

  it("bounds search and result pages and keeps writes approval-scoped", async () => {
    const result = await runtime.run(
      tool("list_reminders").execute({ limit: 101 }).pipe(Effect.result),
    );
    expect(Result.isFailure(result) && result.failure.phase).toBe("input");
    for (const name of [
      "create_reminder",
      "update_reminder",
      "complete_reminder",
      "reopen_reminder",
      "delete_reminder",
    ]) {
      expect(tool(name).policy.recommendedPolicy).toBe("require_approval");
      expect(tool(name).annotations.readOnlyHint).toBe(false);
    }
  });
});
