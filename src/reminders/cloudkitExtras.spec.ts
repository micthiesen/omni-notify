import { Effect, Result } from "effect";
import { describe, expect, it } from "vitest";
import { RemindersCloudKitExtras } from "./cloudkitExtras.js";
import { RemindersError } from "./cloudkit.js";

const reminderId = "Reminder/12345678";
const ruleId = "RecurrenceRule/ABCDEFGH";
const listId = "List/12345678";
const ref = (recordName: string) => ({
  type: "REFERENCE",
  value: { recordName, action: "VALIDATE" },
});
const int = (value: number) => ({ type: "INT64", value });
const str = (value: string) => ({ type: "STRING", value });
type RecordFixture = {
  recordName: string;
  recordType: string;
  recordChangeTag: string;
  fields: Record<string, { type: string; value: unknown }>;
};
function fixture(
  options: {
    recurrence?: boolean;
    mutateAcks?: (records: RecordFixture[]) => unknown[];
    lostResponse?: boolean;
    staleRead?: boolean;
  } = {},
) {
  const records = new Map<string, RecordFixture>([
    [
      listId,
      {
        recordName: listId,
        recordType: "List",
        recordChangeTag: "list-1",
        fields: {
          Name: str("Original"),
          Color: str("blue"),
          HiddenMetadata: int(7),
          ResolutionTokenMap: str(
            JSON.stringify({
              extra: true,
              map: {
                name: { counter: 3, modificationTime: 5, replicaID: "old" },
                color: { counter: 2 },
              },
            }),
          ),
        },
      },
    ],
    [
      reminderId,
      {
        recordName: reminderId,
        recordType: "Reminder",
        recordChangeTag: "reminder-1",
        fields: {
          List: ref(listId),
          RecurrenceRuleIDs: {
            type: "STRING_LIST",
            value: options.recurrence ? ["ABCDEFGH"] : [],
          },
          NotesDocument: str("untouched"),
          ResolutionTokenMap: str(
            JSON.stringify({ map: { titleDocument: { counter: 5 } } }),
          ),
        },
      },
    ],
  ]);
  if (options.recurrence)
    records.set(ruleId, {
      recordName: ruleId,
      recordType: "RecurrenceRule",
      recordChangeTag: "rule-1",
      fields: {
        Reminder: ref(reminderId),
        Frequency: int(0),
        Interval: int(1),
        FirstDayOfTheWeek: int(0),
        Imported: int(0),
        Deleted: int(0),
      },
    });
  const writes: {
    operations: {
      operationType: string;
      record: RecordFixture & { parent?: unknown };
    }[];
    atomic: boolean;
  }[] = [];
  let nextTag = 0;
  const client = new RemindersCloudKitExtras(
    (path, body) =>
      Effect.suspend(() => {
        if (path === "/records/lookup") {
          const id = (body as { records: { recordName: string }[] }).records[0]
            .recordName;
          const record = records.get(id);
          return Effect.succeed({
            records: [
              record
                ? structuredClone(record)
                : { recordName: id, serverErrorCode: "NOT_FOUND" },
            ],
          });
        }
        const request = structuredClone(body) as (typeof writes)[number];
        writes.push(request);
        const acknowledgments = request.operations.map(({ record }) => ({
          ...record,
          recordChangeTag: `ack-${++nextTag}`,
          fields: Object.fromEntries(
            Object.entries(record.fields).map(([name, field]) => [
              name,
              {
                ...field,
                type: field.type ?? (name === "EndDate" ? "TIMESTAMP" : "BYTES"),
              },
            ]),
          ),
        }));
        if (!options.staleRead)
          for (const record of acknowledgments) {
            const old = records.get(record.recordName);
            records.set(record.recordName, {
              ...record,
              fields: { ...old?.fields, ...record.fields },
            });
          }
        if (options.lostResponse)
          return Effect.fail(
            new RemindersError({ operation: "fixture", code: "transport" }),
          );
        return Effect.succeed({
          records: options.mutateAcks?.(acknowledgments) ?? acknowledgments,
        });
      }),
    () =>
      Effect.sync(() =>
        structuredClone(
          [...records.values()].filter(
            (record) => record.recordType === "RecurrenceRule",
          ),
        ),
      ),
  );
  return { client, records, writes };
}

async function fails(effect: Effect.Effect<unknown, unknown>, code?: string) {
  const result = await Effect.runPromise(Effect.result(effect));
  expect(Result.isFailure(result)).toBe(true);
  if (Result.isFailure(result) && code) expect(result.failure).toMatchObject({ code });
}

describe("verified list and recurrence operations", () => {
  it("writes new selectors without guessing types, preserves observed wrappers on update and verifies values", async () => {
    const x = fixture();
    const created = await Effect.runPromise(
      x.client.createRecurrence(reminderId, "reminder-1", ruleId, {
        frequency: "monthly",
        interval: 1,
        endDate: 1800000000000,
        daysOfWeek: [{ dayOfTheWeek: 2, weekNumber: -1 }],
        monthsOfYear: [1, 12],
      }),
    );
    const fields = x.writes[0].operations[1].record.fields;
    expect(fields.EndDate).toEqual({ value: 1800000000000 });
    expect(fields.MonthsOfTheYear).toEqual({
      value: Buffer.from("[1,12]").toString("base64"),
    });
    const updated = await Effect.runPromise(
      x.client.updateRecurrence(
        reminderId,
        created.reminderChangeTag,
        ruleId,
        created.rules[0].recordChangeTag!,
        { endDate: null, monthsOfYear: [2] },
      ),
    );
    expect(x.writes[1].operations[1].record.fields.EndDate).toEqual({
      type: "TIMESTAMP",
      value: null,
    });
    expect(x.writes[1].operations[1].record.fields.MonthsOfTheYear).toEqual({
      type: "BYTES",
      value: Buffer.from("[2]").toString("base64"),
    });
    expect(updated.rules[0]).toMatchObject({
      writable: true,
      recurrence: {
        supported: true,
        rule: {
          endDate: null,
          monthsOfYear: [2],
          daysOfWeek: [{ dayOfTheWeek: 2, weekNumber: -1 }],
        },
      },
    });
  });

  it("renames only Name and merges resolution tokens without touching list metadata or reminders", async () => {
    const x = fixture();
    const reminder = structuredClone(x.records.get(reminderId));
    const result = await Effect.runPromise(
      x.client.updateList(listId, "list-1", "Renamed"),
    );
    expect(result).toMatchObject({
      title: "Renamed",
      color: "blue",
      recordChangeTag: "ack-1",
    });
    expect(x.writes[0].atomic).toBe(true);
    expect(Object.keys(x.writes[0].operations[0].record.fields).sort()).toEqual([
      "Name",
      "ResolutionTokenMap",
    ]);
    const persisted = x.records.get(listId)!;
    expect(persisted.fields.HiddenMetadata).toEqual(int(7));
    expect(
      JSON.parse(persisted.fields.ResolutionTokenMap.value as string),
    ).toMatchObject({
      extra: true,
      map: { name: { counter: 4 }, color: { counter: 2 } },
    });
    expect(x.records.get(reminderId)).toEqual(reminder);
  });

  it("rejects list conflicts and malformed token maps before writing", async () => {
    const x = fixture();
    await fails(x.client.updateList(listId, "wrong", "Renamed"), "conflict");
    x.records.get(listId)!.fields.ResolutionTokenMap = str("not json");
    await fails(x.client.updateList(listId, "list-1", "Renamed"));
    expect(x.writes).toHaveLength(0);
  });

  it("reads exact recurrence details, including opaque first-day zero and unsupported future rules", async () => {
    const x = fixture({ recurrence: true });
    let result = await Effect.runPromise(x.client.getRecurrences(reminderId));
    expect(result.rules[0]).toMatchObject({
      id: ruleId,
      reminderId,
      recordChangeTag: "rule-1",
      writable: true,
      recurrence: {
        supported: true,
        rule: { frequency: "daily", interval: 1, firstDayOfWeek: 0 },
      },
    });
    x.records.get(ruleId)!.fields.Frequency = int(100);
    result = await Effect.runPromise(x.client.getRecurrences(reminderId));
    expect(result.rules[0]).toMatchObject({
      writable: false,
      recurrence: { supported: false, reason: "unknown_frequency" },
    });
  });

  it("creates atomically using stable child identity and verifies both links and fields", async () => {
    const x = fixture();
    const result = await Effect.runPromise(
      x.client.createRecurrence(reminderId, "reminder-1", ruleId, {
        frequency: "daily",
        interval: 2,
        firstDayOfWeek: 0,
      }),
    );
    expect(x.writes).toHaveLength(1);
    expect(x.writes[0].atomic).toBe(true);
    const [parent, child] = x.writes[0].operations;
    expect(parent.record.fields.RecurrenceRuleIDs).toEqual({
      type: "STRING_LIST",
      value: ["ABCDEFGH"],
    });
    expect(child.record.parent).toEqual({ recordName: reminderId });
    expect(child.record.fields).toMatchObject({
      Frequency: int(0),
      Interval: int(2),
      Reminder: ref(reminderId),
    });
    expect(result.rules[0]).toMatchObject({
      id: ruleId,
      recordChangeTag: "ack-2",
      writable: true,
    });
    expect(x.records.get(reminderId)!.fields.NotesDocument).toEqual(str("untouched"));
  });

  it("updates using both change tags then unlinks and soft deletes without completing the reminder", async () => {
    const x = fixture({ recurrence: true });
    const updated = await Effect.runPromise(
      x.client.updateRecurrence(reminderId, "reminder-1", ruleId, "rule-1", {
        frequency: "weekly",
        interval: 3,
      }),
    );
    expect(updated.rules[0].recurrence).toMatchObject({
      supported: true,
      rule: { frequency: "weekly", interval: 3, firstDayOfWeek: 0 },
    });
    const removed = await Effect.runPromise(
      x.client.removeRecurrence(
        reminderId,
        updated.reminderChangeTag,
        ruleId,
        updated.rules[0].recordChangeTag!,
      ),
    );
    expect(removed.rules).toEqual([]);
    expect(x.records.get(ruleId)!.fields.Deleted).toEqual(int(1));
    expect(x.records.get(reminderId)!.fields.RecurrenceRuleIDs.value).toEqual([]);
    expect(x.records.get(reminderId)!.fields).not.toHaveProperty("Completed");
  });

  it.each(["missing", "duplicate", "foreign", "error", "no-tag"])(
    "rejects %s mutation acknowledgments",
    async (kind) => {
      const x = fixture({
        mutateAcks: (acks) => {
          if (kind === "missing") return acks.slice(0, 1);
          if (kind === "duplicate") return [acks[0], acks[0]];
          if (kind === "foreign") return [acks[0], { ...acks[1], recordName: "other" }];
          if (kind === "error")
            return [acks[0], { ...acks[1], serverErrorCode: "PERMISSION_FAILURE" }];
          return [acks[0], { ...acks[1], recordChangeTag: undefined }];
        },
      });
      await fails(
        x.client.createRecurrence(reminderId, "reminder-1", ruleId, {
          frequency: "daily",
          interval: 1,
        }),
      );
      expect(x.writes).toHaveLength(1);
    },
  );

  it("treats a lost response or failed readback as failure without repeating writes", async () => {
    for (const options of [{ lostResponse: true }, { staleRead: true }]) {
      const x = fixture(options);
      await fails(
        x.client.createRecurrence(reminderId, "reminder-1", ruleId, {
          frequency: "daily",
          interval: 1,
        }),
      );
      expect(x.writes).toHaveLength(1);
    }
  });

  it("rejects unknown rules, unlinked relationships, wrong owners and stale tags before mutation", async () => {
    const x = fixture({ recurrence: true });
    await fails(
      x.client.createRecurrence(reminderId, "reminder-1", "RecurrenceRule/IJKLMNOP", {
        frequency: "daily",
        interval: 1,
      }),
      "unsupported",
    );
    await fails(
      x.client.updateRecurrence(reminderId, "reminder-1", ruleId, "wrong", {
        interval: 2,
      }),
      "conflict",
    );
    x.records.get(ruleId)!.fields.FutureUnknownRule = str("opaque");
    await fails(
      x.client.updateRecurrence(reminderId, "reminder-1", ruleId, "rule-1", {
        interval: 2,
      }),
      "unsupported",
    );
    await fails(
      x.client.removeRecurrence(reminderId, "reminder-1", ruleId, "rule-1"),
      "unsupported",
    );
    delete x.records.get(ruleId)!.fields.FutureUnknownRule;
    x.records.get(reminderId)!.fields.RecurrenceRuleIDs.value = [];
    await fails(
      x.client.removeRecurrence(reminderId, "reminder-1", ruleId, "rule-1"),
      "unsupported",
    );
    x.records.get(reminderId)!.fields.RecurrenceRuleIDs.value = ["ABCDEFGH"];
    x.records.get(ruleId)!.fields.Reminder = ref("Reminder/FOREIGN1");
    await fails(x.client.getRecurrences(reminderId));
    expect(x.writes).toHaveLength(0);
  });
});
