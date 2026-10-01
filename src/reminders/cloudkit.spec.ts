import { Effect, Result } from "effect";
import { describe, expect, it } from "vitest";
import { RemindersCloudKitClient, type Reminder } from "./cloudkit.js";
import { decodeCrdtDocument, encodeCrdtDocument } from "./codec.js";

const record = (overrides: Record<string, unknown> = {}) => ({
  recordName: "Reminder/12345678",
  recordType: "Reminder",
  recordChangeTag: "tag-1",
  fields: {
    TitleDocument: { type: "STRING", value: encodeCrdtDocument("Buy milk") },
    NotesDocument: { type: "STRING", value: encodeCrdtDocument("At noon") },
    List: { type: "REFERENCE", value: { recordName: "List/1", action: "VALIDATE" } },
    Completed: { type: "INT64", value: 0 },
    DueDate: { type: "TIMESTAMP", value: 1_699_920_000_000 },
    Priority: { type: "INT64", value: 5 },
    Flagged: { type: "INT64", value: 1 },
    AllDay: { type: "INT64", value: 1 },
  },
  ...overrides,
});

const list = {
  recordName: "List/1",
  recordType: "List",
  fields: {
    Name: { type: "STRING", value: "Groceries" },
    Color: { type: "STRING", value: "blue" },
    Count: { type: "INT64", value: 1 },
  },
};

describe("Reminders CloudKit codec", () => {
  it.each(["TitleDocument", "NotesDocument"])(
    "keeps encrypted %s unavailable",
    async (field) => {
      const r = record();
      const client = new RemindersCloudKitClient(() =>
        Effect.succeed({
          zones: [
            {
              records: [
                {
                  ...r,
                  fields: {
                    ...r.fields,
                    [field]: { type: "ENCRYPTED_BYTES", value: "opaque" },
                  },
                },
              ],
            },
          ],
        }),
      );
      const result = await Effect.runPromise(client.readSnapshot().pipe(Effect.result));
      expect(Result.isFailure(result) && result.failure.code).toBe(
        "awaiting-device-approval",
      );
    },
  );

  it("decodes unencrypted BYTES documents", async () => {
    const r = record();
    const client = new RemindersCloudKitClient(() =>
      Effect.succeed({
        zones: [
          {
            records: [
              {
                ...r,
                fields: {
                  ...r.fields,
                  TitleDocument: { ...r.fields.TitleDocument, type: "BYTES" },
                },
              },
            ],
          },
        ],
      }),
    );
    expect((await Effect.runPromise(client.readSnapshot())).reminders[0]?.title).toBe(
      "Buy milk",
    );
  });

  it("round trips Unicode and rejects malformed documents", () => {
    expect(decodeCrdtDocument(encodeCrdtDocument("Café 🌿\nMilk"))).toBe(
      "Café 🌿\nMilk",
    );
    expect(() => decodeCrdtDocument("%%%")).toThrow();
  });

  it("reads all zone pages and decodes dates, flags, and notes", async () => {
    const requests: unknown[] = [];
    const client = new RemindersCloudKitClient((path, body) => {
      expect(path).toBe("/changes/zone");
      requests.push(body);
      return Effect.succeed({
        zones: [
          {
            records: requests.length === 1 ? [list] : [record()],
            syncToken: `token-${requests.length}`,
            moreComing: requests.length === 1,
          },
        ],
      });
    });
    const snapshot = await Effect.runPromise(client.readSnapshot());
    expect(requests).toHaveLength(2);
    expect(snapshot.lists).toEqual([
      { id: "List/1", title: "Groceries", color: "blue", count: 1 },
    ]);
    expect(snapshot.reminders[0]).toMatchObject({
      title: "Buy milk",
      description: "At noon",
      listId: "List/1",
      dueDate: 1_699_920_000_000,
      priority: 5,
      flagged: true,
      allDay: true,
    });
  });

  it("rejects a stalled cursor rather than returning a partial snapshot", async () => {
    const client = new RemindersCloudKitClient(() =>
      Effect.succeed({
        zones: [{ records: [list], moreComing: true }],
      }),
    );
    await expect(Effect.runPromise(client.readSnapshot())).rejects.toThrow("protocol");
  });

  it.each([
    ["CONFLICT", "conflict"],
    ["UNKNOWN_ERROR", "protocol"],
  ])("rejects partial modify error %s as %s", async (serverErrorCode, expectedCode) => {
    const calls: string[] = [];
    const client = new RemindersCloudKitClient((path) => {
      calls.push(path);
      return Effect.succeed(
        path === "/records/lookup"
          ? { records: [record()] }
          : path === "/changes/zone"
            ? { zones: [{ records: [], moreComing: false }] }
            : {
                records: [
                  {
                    recordName: "Reminder/12345678",
                    recordType: "Reminder",
                    serverErrorCode,
                  },
                ],
              },
      );
    });
    const current = await Effect.runPromise(client.getReminder("Reminder/12345678"));
    expect(current).not.toBeNull();
    await expect(
      Effect.runPromise(client.updateReminder(current as Reminder, { flagged: false })),
    ).rejects.toThrow(expectedCode);
    expect(calls).toEqual([
      "/records/lookup",
      "/changes/zone",
      "/records/lookup",
      "/changes/zone",
      "/records/modify",
    ]);
  });

  it("checks the read-back value after a tagged write", async () => {
    const calls: string[] = [];
    let lookupCount = 0;
    const client = new RemindersCloudKitClient((path) => {
      calls.push(path);
      return Effect.succeed(
        path === "/records/modify"
          ? { records: [{ recordName: "Reminder/12345678", recordChangeTag: "tag-2" }] }
          : path === "/changes/zone"
            ? { zones: [{ records: [], moreComing: false }] }
            : {
                records: [
                  record({ recordChangeTag: ++lookupCount <= 2 ? "tag-1" : "tag-2" }),
                ],
              },
      );
    });
    const current = await Effect.runPromise(client.getReminder("Reminder/12345678"));
    await expect(
      Effect.runPromise(client.updateReminder(current as Reminder, { flagged: false })),
    ).rejects.toThrow("protocol");
    expect(calls).toEqual([
      "/records/lookup",
      "/changes/zone",
      "/records/lookup",
      "/changes/zone",
      "/records/modify",
      "/records/lookup",
      "/changes/zone",
    ]);
  });

  it("rejects mutations of recurring reminders before a write", async () => {
    const calls: string[] = [];
    const client = new RemindersCloudKitClient((path) => {
      calls.push(path);
      return Effect.succeed({
        records: [
          record({
            fields: {
              ...record().fields,
              RecurrenceRules: { type: "STRING", value: "rule" },
            },
          }),
        ],
      });
    });
    const current = await Effect.runPromise(client.getReminder("Reminder/12345678"));
    expect(current?.recurring).toBe(true);
    await expect(
      Effect.runPromise(client.deleteReminder(current as Reminder)),
    ).rejects.toThrow("unsupported");
    expect(calls).toEqual(["/records/lookup"]);
  });

  it("detects a separate recurrence record before a mutation", async () => {
    const calls: string[] = [];
    const client = new RemindersCloudKitClient((path) => {
      calls.push(path);
      return Effect.succeed(
        path === "/records/lookup"
          ? { records: [record()] }
          : {
              zones: [
                {
                  records: [
                    {
                      recordName: "RecurrenceRule/1",
                      recordType: "RecurrenceRule",
                      fields: {
                        Reminder: {
                          type: "REFERENCE",
                          value: { recordName: "Reminder/12345678" },
                        },
                      },
                    },
                  ],
                  moreComing: false,
                },
              ],
            },
      );
    });
    const current = await Effect.runPromise(client.getReminder("Reminder/12345678"));
    expect(current?.recurring).toBe(true);
    await expect(
      Effect.runPromise(client.setCompleted(current as Reminder, true)),
    ).rejects.toThrow("unsupported");
    expect(calls).toEqual(["/records/lookup", "/changes/zone"]);
  });

  it("marks separately referenced recurring reminders in a snapshot", async () => {
    const client = new RemindersCloudKitClient(() =>
      Effect.succeed({
        zones: [
          {
            records: [
              list,
              record(),
              {
                recordName: "RecurrenceRule/1",
                recordType: "RecurrenceRule",
                fields: {
                  Reminder: {
                    type: "REFERENCE",
                    value: { recordName: "Reminder/12345678" },
                  },
                },
              },
            ],
          },
        ],
      }),
    );
    const snapshot = await Effect.runPromise(client.readSnapshot());
    expect(snapshot.reminders[0].recurring).toBe(true);
  });

  it("rejects an existing create ID whose notes or dates differ", async () => {
    const calls: string[] = [];
    const client = new RemindersCloudKitClient((path) => {
      calls.push(path);
      return Effect.succeed(
        path === "/records/lookup"
          ? { records: [record()] }
          : { zones: [{ records: [], moreComing: false }] },
      );
    });
    await expect(
      Effect.runPromise(
        client.createReminder({
          id: "Reminder/12345678",
          listId: "List/1",
          title: "Buy milk",
          description: "Different notes",
          dueDate: 1_699_920_000_000,
          priority: 5,
          flagged: true,
          allDay: true,
        }),
      ),
    ).rejects.toThrow("conflict");
    expect(calls).toEqual(["/records/lookup", "/changes/zone"]);
  });

  it("requires UTC midnight for an all-day civil date", async () => {
    const client = new RemindersCloudKitClient(() => {
      throw new Error("network should not be reached");
    });
    await expect(
      Effect.runPromise(
        client.createReminder({
          id: "Reminder/12345678",
          listId: "List/1",
          title: "Buy milk",
          allDay: true,
          dueDate: 1_699_920_000_001,
        }),
      ),
    ).rejects.toThrow("invalid");
  });

  it("updates only requested fields and preserves an unknown timezone field", async () => {
    let modifiedFields: Record<string, unknown> | undefined;
    let written = false;
    const original = record({
      fields: {
        ...record().fields,
        TimeZone: { type: "STRING", value: "America/Vancouver" },
      },
    });
    const client = new RemindersCloudKitClient((path, body) => {
      if (path === "/changes/zone") return Effect.succeed({ zones: [{ records: [] }] });
      if (path === "/records/modify") {
        modifiedFields = (
          body as { operations: { record: { fields: Record<string, unknown> } }[] }
        ).operations[0].record.fields;
        written = true;
        return Effect.succeed({
          records: [{ recordName: "Reminder/12345678", recordChangeTag: "tag-2" }],
        });
      }
      return Effect.succeed({
        records: [
          written
            ? record({
                ...original,
                recordChangeTag: "tag-2",
                fields: {
                  ...original.fields,
                  Flagged: { type: "INT64", value: 0 },
                },
              })
            : original,
        ],
      });
    });
    const current = await Effect.runPromise(client.getReminder("Reminder/12345678"));
    const updated = await Effect.runPromise(
      client.updateReminder(current as Reminder, { flagged: false }),
    );
    expect(updated.flagged).toBe(false);
    expect(modifiedFields).toHaveProperty("Flagged");
    expect(modifiedFields).not.toHaveProperty("TimeZone");
    expect(modifiedFields).not.toHaveProperty("DueDate");
  });
});
