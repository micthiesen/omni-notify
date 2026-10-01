import { Effect, Result } from "effect";
import { afterAll, describe, expect, it } from "vitest";
import { createMitoolsTestRuntime } from "../../test/mitools.js";
import type { Reminder } from "../../reminders/cloudkit.js";
import type { McpRuntime } from "../runtime.js";
import { createRemindersTools } from "./reminders.js";

const tools = createRemindersTools({} as McpRuntime);
const runtime = createMitoolsTestRuntime();
afterAll(() => runtime.dispose());
const tool = (name: string) => tools.find((item) => item.name === name)!;

describe("server Reminders MCP boundary", () => {
  it("keeps list counts and filtered reminder totals independent of pagination", async () => {
    const lists = Array.from({ length: 8 }, (_, i) => ({
      id: `List/${i}`,
      title: `List ${i}`,
      color: null,
      count: i === 0 ? 8 : 3,
    }));
    const reminders: Reminder[] = lists.flatMap((list) =>
      Array.from({ length: list.count + 1 }, (_, i) => ({
        id: `Reminder/${list.id}/${i}`,
        listId: list.id,
        title: `Item ${i}`,
        description: "",
        completed: i === list.count,
        completedDate: null,
        dueDate: null,
        startDate: null,
        priority: 0,
        flagged: false,
        allDay: false,
        deleted: false,
        createdDate: null,
        lastModifiedDate: null,
        recordChangeTag: "tag",
        recurring: false,
      })),
    );
    reminders.push({ ...reminders[0], id: "Reminder/deleted", deleted: true });
    const reads = createRemindersTools({
      reminders: { snapshot: () => Effect.succeed({ lists, reminders }) },
    } as unknown as McpRuntime);
    const execute = (name: string, input: Record<string, unknown>) =>
      runtime.run(reads.find((item) => item.name === name)!.execute(input));
    expect(await execute("list_reminder_lists", { limit: 1 })).toEqual({
      items: [lists[0]],
      total: 8,
      nextCursor: 1,
    });
    expect(await execute("list_reminder_lists", { cursor: 7, limit: 1 })).toEqual({
      items: [lists[7]],
      total: 8,
      nextCursor: null,
    });
    expect(
      await execute("list_reminders", { completed: false, limit: 1 }),
    ).toMatchObject({ total: 29, nextCursor: 1 });
    expect(
      await execute("list_reminders", { listId: "List/0", completed: false, limit: 1 }),
    ).toMatchObject({ total: 8, nextCursor: 1 });
    expect(
      await execute("list_reminders", { listId: "List/0", completed: true, limit: 1 }),
    ).toMatchObject({ total: 1, nextCursor: null });
    expect(
      await execute("list_reminders", { listId: "List/0", limit: 1 }),
    ).toMatchObject({ total: 9, nextCursor: 1 });
  });

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
      "update_reminder_list",
      "create_reminder_recurrence",
      "update_reminder_recurrence",
      "remove_reminder_recurrence",
    ]) {
      expect(tool(name).policy.recommendedPolicy).toBe("require_approval");
      expect(tool(name).annotations.readOnlyHint).toBe(false);
    }
  });

  it("rejects unsupported list writes and missing rule concurrency identity at the MCP boundary", async () => {
    for (const [name, input] of [
      [
        "update_reminder_list",
        {
          id: "List/test",
          changeTag: "tag",
          idempotencyKey: "fixture-key-123456",
          color: "red",
        },
      ],
      [
        "update_reminder_recurrence",
        {
          id: "Reminder/test",
          changeTag: "tag",
          idempotencyKey: "fixture-key-123456",
          ruleId: "RecurrenceRule/test",
          patch: { interval: 2 },
        },
      ],
      [
        "create_reminder_recurrence",
        {
          id: "Reminder/test",
          changeTag: "tag",
          idempotencyKey: "fixture-key-123456",
          rule: { frequency: "not-a-frequency", interval: 1 },
        },
      ],
    ] as const) {
      const result = await runtime.run(tool(name).execute(input).pipe(Effect.result));
      expect(Result.isFailure(result) && result.failure.phase).toBe("input");
    }
    expect(tools.some((item) => item.name === "create_reminder_list")).toBe(false);
    expect(tools.some((item) => item.name === "delete_reminder_list")).toBe(false);
  });
});
