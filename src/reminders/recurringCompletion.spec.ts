import { Effect, Result } from "effect";
import { describe, expect, it, vi } from "vitest";
import {
  requestRecurringCompletion,
  completeRecurringOccurrence,
} from "./recurringCompletion.js";
import { RemindersError } from "./cloudkit.js";
const input = {
  reminderId: "Reminder/fixture",
  ruleId: "RecurrenceRule/fixture",
  timeZone: "America/Vancouver",
};
const record = {
  recordName: input.reminderId,
  recordType: "Reminder",
  recordChangeTag: "new",
};

describe("single-request recurring completion transport", () => {
  it("uses Apple's mutating query and returns unverified records for fresh verification", async () => {
    const post = vi.fn(() => Effect.succeed({ records: [record] }));
    expect(await Effect.runPromise(requestRecurringCompletion(post, input))).toEqual({
      verified: false,
      records: [record],
    });
    expect(post).toHaveBeenCalledTimes(1);
    expect(post.mock.calls[0]).toEqual([
      "/records/query",
      {
        zoneID: { zoneName: "Reminders", zoneType: "REGULAR_CUSTOM_ZONE" },
        query: {
          recordType: "CompleteRecurringReminder",
          filterBy: [
            {
              comparator: "EQUALS",
              fieldName: "Reminder",
              fieldValue: {
                type: "REFERENCE",
                value: { recordName: input.reminderId, action: "VALIDATE" },
              },
            },
            {
              comparator: "EQUALS",
              fieldName: "RecurrenceRule",
              fieldValue: {
                type: "REFERENCE",
                value: { recordName: input.ruleId, action: "VALIDATE" },
              },
            },
            {
              comparator: "EQUALS",
              fieldName: "TimeZone",
              fieldValue: { type: "STRING", value: "America/Vancouver" },
            },
          ],
        },
      },
    ]);
  });

  it.each([
    { records: [] },
    { records: [record, record] },
    { records: [record], continuationMarker: "next" },
    { records: [{ ...record, serverErrorCode: "CONFLICT" }] },
    {
      records: Array.from({ length: 101 }, (_, i) => ({
        ...record,
        recordName: `Reminder/${i}`,
      })),
    },
  ])(
    "rejects incomplete or malformed response without repeating the query",
    async (response) => {
      const post = vi.fn(() => Effect.succeed(response));
      expect(
        Result.isFailure(
          await Effect.runPromise(
            requestRecurringCompletion(post, input).pipe(Effect.result),
          ),
        ),
      ).toBe(true);
      expect(post).toHaveBeenCalledTimes(1);
    },
  );

  it("never retries a lost response", async () => {
    const post = vi.fn(() =>
      Effect.fail(new RemindersError({ operation: "fixture", code: "transport" })),
    );
    expect(
      Result.isFailure(
        await Effect.runPromise(
          requestRecurringCompletion(post, input).pipe(Effect.result),
        ),
      ),
    ).toBe(true);
    expect(post).toHaveBeenCalledTimes(1);
  });

  it("validates identities and explicit timezone before mutation", async () => {
    for (const patch of [
      { timeZone: "invalid zone" },
      { reminderId: "List/fixture" },
      { ruleId: "Reminder/fixture" },
    ]) {
      const post = vi.fn(() => Effect.succeed({ records: [record] }));
      expect(
        Result.isFailure(
          await Effect.runPromise(
            requestRecurringCompletion(post, { ...input, ...patch }).pipe(
              Effect.result,
            ),
          ),
        ),
      ).toBe(true);
      expect(post).not.toHaveBeenCalled();
    }
  });
});

function completionFixture() {
  const due = 1793466000000;
  const before: import("./cloudkit.js").Reminder = {
    id: input.reminderId,
    listId: "List/fixture",
    title: "Synthetic",
    description: "Own test only",
    completed: false,
    completedDate: null,
    dueDate: due,
    startDate: null,
    priority: 5,
    flagged: true,
    allDay: false,
    deleted: false,
    createdDate: 1,
    lastModifiedDate: 2,
    recordChangeTag: "before",
    recurring: true,
  };
  const current = {
    ...before,
    dueDate: due + 25 * 60 * 60 * 1000,
    recordChangeTag: "after",
  };
  const completed = {
    ...before,
    id: "Reminder/completed-copy",
    completed: true,
    completedDate: Date.now(),
    recordChangeTag: "copy",
    recurring: false,
  };
  let called = false;
  const afterRule: import("./cloudkitExtras.js").ReminderRecurrences = {
    reminderId: before.id,
    reminderChangeTag: current.recordChangeTag,
    rules: [
      {
        id: input.ruleId,
        reminderId: before.id,
        recordChangeTag: "rule-tag",
        writable: true,
        recurrence: {
          supported: true,
          rule: { frequency: "daily", interval: 1, firstDayOfWeek: 0 },
        },
      },
    ],
  };
  const related = structuredClone({
    ...afterRule,
    reminderChangeTag: before.recordChangeTag,
  });
  const returned = [
    {
      recordName: current.id,
      recordType: "Reminder",
      recordChangeTag: current.recordChangeTag,
    },
    {
      recordName: completed.id,
      recordType: "Reminder",
      recordChangeTag: completed.recordChangeTag,
    },
    { recordName: before.listId, recordType: "List", recordChangeTag: "list" },
  ];
  const post = vi.fn(() =>
    Effect.sync(() => {
      called = true;
      return { records: returned };
    }),
  );
  const deps = {
    post,
    getReminder: (id: string) =>
      Effect.succeed(
        id === before.id
          ? called
            ? current
            : before
          : id === completed.id
            ? completed
            : null,
      ),
    getRecurrences: () => Effect.succeed(called ? afterRule : related),
    invalidate: vi.fn(() => Effect.void),
  };
  return {
    before,
    current,
    completed,
    afterRule,
    related,
    returned,
    deps,
    target: {
      id: before.id,
      changeTag: before.recordChangeTag,
      ruleId: input.ruleId,
      ruleChangeTag: "rule-tag",
      timeZone: input.timeZone,
    },
  };
}

describe("verified recurring occurrence advancement", () => {
  it.each([
    ["advanced", "tag"],
    ["ended", "tag"],
    ["advanced", "writable"],
    ["ended", "writable"],
  ] as const)(
    "keeps %s uncertain when fresh rule %s changes without decoded value changes",
    async (state, field) => {
      const x = completionFixture();
      if (state === "ended") {
        x.returned.splice(1, 1);
        Object.assign(x.current, {
          dueDate: x.before.dueDate,
          completed: true,
          completedDate: Date.now(),
        });
        const recurrence = {
          supported: true,
          rule: {
            frequency: "daily",
            interval: 1,
            endDate: x.before.dueDate! + 60_000,
          },
        };
        Object.assign(x.related.rules[0], { recurrence });
        Object.assign(x.afterRule.rules[0], { recurrence });
      }
      Object.assign(
        x.afterRule.rules[0],
        field === "tag"
          ? { recordChangeTag: "concurrently-changed" }
          : { writable: false },
      );
      const result = await Effect.runPromise(
        completeRecurringOccurrence(x.deps, x.target).pipe(Effect.result),
      );
      expect(Result.isFailure(result) && result.failure.code).toBe("uncertain");
      expect(x.deps.post).toHaveBeenCalledTimes(1);
    },
  );

  it("verifies a simple finite daily series ending on this occurrence's civil date", async () => {
    const x = completionFixture();
    x.returned.splice(1, 1);
    Object.assign(x.current, {
      dueDate: x.before.dueDate,
      completed: true,
      completedDate: Date.now(),
    });
    const recurrence = {
      supported: true,
      rule: { frequency: "daily", interval: 1, endDate: x.before.dueDate! + 60_000 },
    };
    Object.assign(x.related.rules[0], { recurrence });
    Object.assign(x.afterRule.rules[0], { recurrence });
    expect(
      await Effect.runPromise(completeRecurringOccurrence(x.deps, x.target)),
    ).toMatchObject({
      state: "ended",
      verified: true,
      nextDueDate: null,
      completedReminderId: x.before.id,
    });
  });

  it.each(["later-end-date", "count-only", "hourly", "selectors"])(
    "does not infer series exhaustion for %s",
    async (kind) => {
      const x = completionFixture();
      x.returned.splice(1, 1);
      Object.assign(x.current, {
        dueDate: x.before.dueDate,
        completed: true,
        completedDate: Date.now(),
      });
      const rule: Record<string, unknown> = {
        frequency: "daily",
        interval: 1,
        endDate: x.before.dueDate! + 60_000,
      };
      if (kind === "later-end-date") rule.endDate = x.before.dueDate! + 7 * 86400000;
      if (kind === "count-only") {
        delete rule.endDate;
        rule.occurrenceCount = 1;
      }
      if (kind === "hourly") rule.frequency = "hourly";
      if (kind === "selectors") rule.daysOfMonth = [1];
      Object.assign(x.related.rules[0], { recurrence: { supported: true, rule } });
      Object.assign(x.afterRule.rules[0], { recurrence: { supported: true, rule } });
      const result = await Effect.runPromise(
        completeRecurringOccurrence(x.deps, x.target).pipe(Effect.result),
      );
      expect(Result.isFailure(result) && result.failure.code).toBe("uncertain");
      expect(x.deps.post).toHaveBeenCalledTimes(1);
    },
  );

  it("verifies same-identity advancement and completed copy across the observed 25-hour DST transition", async () => {
    const x = completionFixture();
    const result = await Effect.runPromise(
      completeRecurringOccurrence(x.deps, x.target),
    );
    expect(result).toMatchObject({
      state: "advanced",
      verified: true,
      reminderId: x.before.id,
      completedReminderId: x.completed.id,
      previousDueDate: x.before.dueDate,
      nextDueDate: x.current.dueDate,
    });
    expect(x.deps.post).toHaveBeenCalledTimes(1);
    expect(x.deps.invalidate).toHaveBeenCalledTimes(1);
  });

  it.each([
    "stale-reminder",
    "stale-rule",
    "already-completed",
    "unknown-rule",
    "no-due",
    "invalid-zone",
  ])("rejects preflight %s before querying", async (kind) => {
    const x = completionFixture();
    if (kind === "stale-reminder") x.target.changeTag = "wrong";
    if (kind === "stale-rule") x.target.ruleChangeTag = "wrong";
    if (kind === "already-completed") Object.assign(x.before, { completed: true });
    if (kind === "unknown-rule") Object.assign(x.related.rules[0], { writable: false });
    if (kind === "no-due") Object.assign(x.before, { dueDate: null });
    if (kind === "invalid-zone") x.target.timeZone = "not a zone";
    expect(
      Result.isFailure(
        await Effect.runPromise(
          completeRecurringOccurrence(x.deps, x.target).pipe(Effect.result),
        ),
      ),
    ).toBe(true);
    expect(x.deps.post).not.toHaveBeenCalled();
  });

  it.each([
    "missing-copy",
    "unchanged-due",
    "root-completed",
    "changed-content",
    "changed-list",
    "copy-incomplete",
    "copy-recurring",
    "old-completion",
    "wrong-copy-due",
    "tag-raced",
    "rule-changed",
  ])("keeps %s outcomes uncertain after exactly one query", async (kind) => {
    const x = completionFixture();
    if (kind === "missing-copy") x.returned.splice(1, 1);
    if (kind === "unchanged-due") x.current.dueDate = x.before.dueDate!;
    if (kind === "root-completed") x.current.completed = true;
    if (kind === "changed-content") x.current.description = "Unexpected edit";
    if (kind === "changed-list") x.completed.listId = "List/other";
    if (kind === "copy-incomplete") x.completed.completed = false;
    if (kind === "copy-recurring") x.completed.recurring = true;
    if (kind === "old-completion") x.completed.completedDate = 1;
    if (kind === "wrong-copy-due") x.completed.dueDate = 1;
    if (kind === "tag-raced") x.current.recordChangeTag = "concurrent";
    if (kind === "rule-changed")
      Object.assign(x.afterRule.rules[0], {
        recurrence: { supported: true, rule: { frequency: "weekly", interval: 1 } },
      });
    const result = await Effect.runPromise(
      completeRecurringOccurrence(x.deps, x.target).pipe(Effect.result),
    );
    expect(Result.isFailure(result) && result.failure.code).toBe("uncertain");
    expect(x.deps.post).toHaveBeenCalledTimes(1);
  });
});
