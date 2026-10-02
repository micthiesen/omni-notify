import { Effect, Result } from "effect";
import { describe, expect, it, vi } from "vitest";
import {
  requestRecurringCompletion,
  completeRecurringOccurrence,
} from "./recurringCompletion.js";
import { RemindersCloudKitClient, RemindersError } from "./cloudkit.js";
import { encodeCrdtDocument } from "./codec.js";
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
  const previewFields = (reminder: typeof before) => ({
    Completed: { type: "INT64", value: reminder.completed ? 1 : 0 },
    DueDate: { type: "TIMESTAMP", value: reminder.dueDate },
    ...(reminder.completedDate !== null
      ? { CompletionDate: { type: "TIMESTAMP", value: reminder.completedDate } }
      : {}),
  });
  const returned = [
    {
      recordName: current.id,
      recordType: "Reminder",
      recordChangeTag: before.recordChangeTag,
      fields: previewFields(current),
    },
    {
      recordName: completed.id,
      recordType: "Reminder",
      recordChangeTag: before.recordChangeTag,
      fields: previewFields(completed),
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
  };
  return {
    before,
    current,
    completed,
    afterRule,
    related,
    returned,
    previewFields,
    deps,
    target: {
      id: before.id,
      changeTag: before.recordChangeTag,
      ruleId: input.ruleId,
      ruleChangeTag: "rule-tag",
      timeZone: "America/Los_Angeles",
    },
  };
}

describe("verified recurring occurrence advancement", () => {
  it("rejects Apple's obsolete Vancouver fall-back adjustment without repairing or repeating it", async () => {
    const x = completionFixture();
    x.target.timeZone = "America/Vancouver";
    // tzdata 2026b: Vancouver stays on UTC-7. A 25-hour advance moves 10am to 11am.
    const result = await Effect.runPromise(
      completeRecurringOccurrence(x.deps, x.target).pipe(Effect.result),
    );
    expect(Result.isFailure(result) && result.failure.code).toBe("uncertain");
    expect(x.deps.post).toHaveBeenCalledTimes(1);
  });

  it.each([
    ["America/Vancouver", 1793466000000, 24],
    ["America/Los_Angeles", Date.UTC(2026, 2, 7, 18), 23],
  ] as const)(
    "accepts %s advancement preserving local time over %s + %s hours",
    async (timeZone, due, hours) => {
      const x = completionFixture();
      x.target.timeZone = timeZone;
      Object.assign(x.before, { dueDate: due });
      x.current.dueDate = due + hours * 3600000;
      x.completed.dueDate = due;
      x.returned[0].fields = x.previewFields(x.current);
      x.returned[1].fields = x.previewFields(x.completed);
      expect(
        await Effect.runPromise(completeRecurringOccurrence(x.deps, x.target)),
      ).toMatchObject({ state: "advanced", verified: true });
    },
  );

  it("leaves all-day civil-date verification separate from timed local-clock checks", async () => {
    const x = completionFixture();
    const due = Date.UTC(2026, 10, 1);
    Object.assign(x.before, { dueDate: due, allDay: true });
    Object.assign(x.current, { dueDate: due + 86400000, allDay: true });
    Object.assign(x.completed, { dueDate: due, allDay: true });
    x.returned[0].fields = x.previewFields(x.current);
    x.returned[1].fields = x.previewFields(x.completed);
    expect(
      await Effect.runPromise(completeRecurringOccurrence(x.deps, x.target)),
    ).toMatchObject({ state: "advanced", verified: true });
  });

  it.each([
    "wrong-completed-type",
    "wrong-due-preview",
    "missing-copy-completion",
    "unchanged-root-tag",
    "clone-raced",
  ])("keeps %s uncertain without trusting preview change tags", async (kind) => {
    const x = completionFixture();
    if (kind === "wrong-completed-type")
      x.returned[0].fields!.Completed.type = "STRING";
    if (kind === "wrong-due-preview") x.returned[0].fields!.DueDate.value = 1;
    if (kind === "missing-copy-completion") delete x.returned[1].fields!.CompletionDate;
    if (kind === "unchanged-root-tag") {
      x.current.recordChangeTag = x.before.recordChangeTag;
      Object.assign(x.afterRule, { reminderChangeTag: x.before.recordChangeTag });
    }
    if (kind === "clone-raced") {
      const get = x.deps.getReminder;
      let cloneReads = 0;
      x.deps.getReminder = (id) =>
        id === x.completed.id && ++cloneReads > 1
          ? Effect.succeed({ ...x.completed, recordChangeTag: "raced" })
          : get(id);
    }
    const result = await Effect.runPromise(
      completeRecurringOccurrence(x.deps, x.target).pipe(Effect.result),
    );
    expect(Result.isFailure(result) && result.failure.code).toBe("uncertain");
    expect(x.deps.post).toHaveBeenCalledTimes(1);
  });

  it("retains the indexed snapshot and discovers the completed clone through incremental refresh", async () => {
    const int = (value: number) => ({ type: "INT64", value });
    const stamp = (value: number) => ({ type: "TIMESTAMP", value });
    const ref = (recordName: string) => ({
      type: "REFERENCE",
      value: { recordName, action: "VALIDATE" },
    });
    const listId = "List/fixture";
    const oldDue = 1793466000000;
    const list = {
      recordName: listId,
      recordType: "List",
      fields: { Name: { type: "STRING", value: "Synthetic" } },
    };
    const original = {
      recordName: input.reminderId,
      recordType: "Reminder",
      recordChangeTag: "before",
      fields: {
        TitleDocument: {
          type: "STRING",
          value: encodeCrdtDocument("Own synthetic reminder"),
        },
        List: ref(listId),
        Completed: int(0),
        DueDate: stamp(oldDue),
        RecurrenceRuleIDs: { type: "STRING_LIST", value: ["ABCDEFGH"] },
      },
    };
    const ruleId = "RecurrenceRule/ABCDEFGH";
    const rule = {
      recordName: ruleId,
      recordType: "RecurrenceRule",
      recordChangeTag: "rule-tag",
      fields: { Reminder: ref(input.reminderId), Frequency: int(0), Interval: int(1) },
    };
    const root = {
      ...original,
      recordChangeTag: "advanced",
      fields: { ...original.fields, DueDate: stamp(oldDue + 25 * 3600000) },
    };
    const clone = {
      ...original,
      recordName: "Reminder/completed-copy",
      recordChangeTag: "clone",
      fields: {
        ...original.fields,
        Completed: int(1),
        CompletionDate: stamp(Date.now()),
        RecurrenceRuleIDs: { type: "STRING_LIST", value: [] },
      },
    };
    let mutated = false;
    let mutationCalls = 0;
    let zoneCalls = 0;
    const client = new RemindersCloudKitClient((path, body) =>
      Effect.sync(() => {
        if (path === "/changes/zone")
          return {
            zones: [
              {
                records: ++zoneCalls === 1 ? [list] : mutated ? [root, clone] : [],
                syncToken: `token-${zoneCalls}`,
              },
            ],
          };
        if (path === "/records/lookup") {
          const id = (body as { records: { recordName: string }[] }).records[0]
            .recordName;
          return {
            records: [
              id === ruleId
                ? rule
                : id === clone.recordName
                  ? clone
                  : mutated
                    ? root
                    : original,
            ],
          };
        }
        if (
          (body as { query: { recordType: string } }).query.recordType ===
          "CompleteRecurringReminder"
        ) {
          mutated = true;
          mutationCalls++;
          return {
            records: [
              { ...root, recordChangeTag: "before" },
              { ...clone, recordChangeTag: "before" },
              list,
            ],
          };
        }
        return { records: mutated ? [root, clone, rule] : [original, rule] };
      }),
    );
    await Effect.runPromise(client.readSnapshot());
    expect(client.hasSnapshot()).toBe(true);
    const receipt = await Effect.runPromise(
      client.completeRecurring({
        id: input.reminderId,
        changeTag: "before",
        ruleId,
        ruleChangeTag: "rule-tag",
        timeZone: "America/Los_Angeles",
      }),
    );
    expect(receipt).toMatchObject({
      state: "advanced",
      reminderChangeTag: "advanced",
      completedReminderChangeTag: "clone",
    });
    expect(client.hasSnapshot()).toBe(true);
    expect(zoneCalls).toBeGreaterThan(1);
    expect(mutationCalls).toBe(1);
  });

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
        x.returned[0].fields = x.previewFields(x.current);
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
    x.returned[0].fields = x.previewFields(x.current);
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
      x.returned[0].fields = x.previewFields(x.current);
      const result = await Effect.runPromise(
        completeRecurringOccurrence(x.deps, x.target).pipe(Effect.result),
      );
      expect(Result.isFailure(result) && result.failure.code).toBe("uncertain");
      expect(x.deps.post).toHaveBeenCalledTimes(1);
    },
  );

  it("verifies same-identity advancement and completed copy across the 25-hour Los Angeles DST transition", async () => {
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
    expect(x.returned[0].recordChangeTag).toBe(x.before.recordChangeTag);
    expect(x.returned[1].recordChangeTag).toBe(x.before.recordChangeTag);
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
