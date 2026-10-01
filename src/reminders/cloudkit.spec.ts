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

function snapshotClient(
  response: () => Effect.Effect<{ zones: { records: unknown[] }[] }>,
) {
  return new RemindersCloudKitClient((path) =>
    path === "/changes/zone"
      ? Effect.succeed({ zones: [{ records: [list] }] })
      : response().pipe(Effect.map((value) => ({ records: value.zones[0].records }))),
  );
}

describe("Reminders CloudKit codec", () => {
  it.each([
    ["RecurrenceRuleIDs", "STRING_LIST", ["RecurrenceRule/1"]],
    ["RecurrenceRuleIDs", "UNKNOWN_LIST", ["RecurrenceRule/1"]],
    ["RecurrenceRuleIDs", "STRING_LIST", null],
    ["RecurrenceRuleIDs", "UNKNOWN_LIST", ""],
    ["RecurrenceRuleIDs", "STRING", []],
    ["RecurrenceRules", "UNKNOWN_LIST", []],
  ])(
    "keeps populated, malformed, or unknown recurrence fields read-only: %s %s %j",
    async (key, type, value) => {
      const calls: string[] = [];
      const client = new RemindersCloudKitClient((path) => {
        calls.push(path);
        return Effect.succeed({
          records: [
            record({
              fields: { ...record().fields, [key as string]: { type, value } },
            }),
          ],
        });
      });
      const current = await Effect.runPromise(client.getReminder("Reminder/12345678"));
      expect(current?.recurring).toBe(true);
      await expect(
        Effect.runPromise(
          client.updateReminder(current as Reminder, { flagged: false }),
        ),
      ).rejects.toThrow("unsupported");
      expect(calls).toEqual(["/records/lookup"]);
    },
  );

  it("requests at most 50 reminders on every compound query page", async () => {
    let pages = 0;
    const limits: number[] = [];
    const client = new RemindersCloudKitClient((path, body) => {
      if (path === "/changes/zone")
        return Effect.succeed({ zones: [{ records: [list] }] });
      limits.push((body as { resultsLimit: number }).resultsLimit);
      return Effect.succeed({
        records: [],
        ...(++pages < 3 ? { continuationMarker: `page-${pages}` } : {}),
      });
    });
    await Effect.runPromise(client.readSnapshot());
    expect(limits).toEqual([50, 50, 50]);
  });

  it("caches complete list discovery and preserves it after an incomplete refresh", async () => {
    const cursors: unknown[] = [];
    let reads = 0;
    const client = new RemindersCloudKitClient((path, body) => {
      if (path !== "/changes/zone") return Effect.succeed({ records: [] });
      cursors.push((body as { zones: { syncToken?: string }[] }).zones[0].syncToken);
      const responses = [
        { records: [list], syncToken: "stable" },
        {
          records: [{ recordName: "List/1", deleted: true }],
          moreComing: true,
          syncToken: "partial",
        },
        { error: { serverErrorCode: "ERROR" } },
        { records: [], syncToken: "done" },
        {
          records: [
            {
              recordName: "List/1",
              recordType: "List",
              fields: { Deleted: { type: "INT64", value: 1 } },
            },
          ],
          syncToken: "deleted",
        },
      ];
      return Effect.succeed({ zones: [responses[reads++]] });
    });
    await Effect.runPromise(client.readSnapshot());
    await expect(Effect.runPromise(client.readSnapshot())).rejects.toThrow("protocol");
    expect((await Effect.runPromise(client.readSnapshot())).lists).toHaveLength(1);
    expect((await Effect.runPromise(client.readSnapshot())).lists).toHaveLength(0);
    expect(cursors).toEqual([undefined, "stable", "partial", "stable", "done"]);
  });

  it("allows long list history without consuming the current-record query budget", async () => {
    let pages = 0;
    const client = new RemindersCloudKitClient((path) =>
      Effect.succeed(
        path === "/changes/zone"
          ? {
              zones: [
                {
                  records: ++pages === 82 ? [list] : [],
                  syncToken: `cursor-${pages}`,
                  moreComing: pages < 82,
                },
              ],
            }
          : { records: [record()] },
      ),
    );
    expect((await Effect.runPromise(client.readSnapshot())).reminders).toHaveLength(1);
    expect(pages).toBe(82);
  });

  it("caps incomplete list discovery and retries from its original cursor", async () => {
    let pages = 0;
    let lastCursor: unknown;
    const client = new RemindersCloudKitClient((_path, body) => {
      lastCursor = (body as { zones: { syncToken?: string }[] }).zones[0].syncToken;
      return Effect.succeed({
        zones: [
          { records: [], syncToken: `cursor-${++pages}`, moreComing: pages <= 200 },
        ],
      });
    });
    await expect(Effect.runPromise(client.readSnapshot())).rejects.toThrow("protocol");
    expect(pages).toBe(200);
    expect((await Effect.runPromise(client.readSnapshot())).lists).toEqual([]);
    expect(lastCursor).toBeUndefined();
  });

  it("probes newest reminders without scanning the entire history or updating the list cursor", async () => {
    const bodies: unknown[] = [];
    const client = new RemindersCloudKitClient((_path, body) => {
      bodies.push(body);
      return Effect.succeed({
        zones: [
          {
            records: bodies.length === 1 ? [list, record()] : [],
            moreComing: bodies.length === 1,
            syncToken: "probe-only",
          },
        ],
      });
    });
    await Effect.runPromise(client.verifyReadAccess());
    expect(bodies).toHaveLength(1);
    expect(bodies[0]).toMatchObject({
      zones: [{ reverse: true, desiredRecordTypes: ["List", "Reminder"] }],
    });
    await Effect.runPromise(client.readSnapshot());
    expect(bodies[1]).toMatchObject({ zones: [{ desiredRecordTypes: ["List"] }] });
    expect((bodies[1] as { zones: unknown[] }).zones[0]).not.toHaveProperty(
      "syncToken",
    );
  });

  it("does not use list names or a single readable reminder to claim protected access", async () => {
    let pages = 0;
    const client = new RemindersCloudKitClient(() =>
      Effect.succeed({
        zones: [
          {
            records:
              ++pages === 1
                ? [list]
                : [
                    record(),
                    record({
                      recordName: "encrypted",
                      fields: {
                        ...record().fields,
                        TitleDocument: { type: "ENCRYPTED_BYTES", value: "opaque" },
                      },
                    }),
                  ],
            moreComing: true,
            syncToken: "next",
          },
        ],
      }),
    );
    const result = await Effect.runPromise(
      client.verifyReadAccess().pipe(Effect.result),
    );
    expect(Result.isFailure(result) && result.failure.code).toBe(
      "awaiting-device-approval",
    );
    expect(pages).toBe(2);
  });

  it("accepts a verified empty account but rejects an incomplete access probe", async () => {
    const empty = new RemindersCloudKitClient(() =>
      Effect.succeed({ zones: [{ records: [] }] }),
    );
    await expect(Effect.runPromise(empty.verifyReadAccess())).resolves.toBeUndefined();
    const incomplete = new RemindersCloudKitClient(() =>
      Effect.succeed({ zones: [{ records: [list], moreComing: true }] }),
    );
    await expect(Effect.runPromise(incomplete.verifyReadAccess())).rejects.toThrow(
      "protocol",
    );
    const missing = new RemindersCloudKitClient(() => Effect.succeed({ zones: [{}] }));
    await expect(Effect.runPromise(missing.verifyReadAccess())).rejects.toThrow(
      "protocol",
    );
  });

  it("pages list discovery and current records, excluding stale, deleted, and other-list reminders", async () => {
    const calls: { path: string; body: unknown }[] = [];
    const stale = record({
      fields: {
        ...record().fields,
        TitleDocument: { type: "STRING", value: encodeCrdtDocument("Stale") },
      },
    });
    const client = new RemindersCloudKitClient((path, body) => {
      calls.push({ path, body });
      if (path === "/changes/zone")
        return Effect.succeed({
          zones: [
            {
              records: calls.length === 1 ? [list, stale] : [],
              moreComing: calls.length === 1,
              syncToken: "lists-next",
            },
          ],
        });
      const cursor = (body as { continuationMarker?: string }).continuationMarker;
      return Effect.succeed(
        cursor
          ? {
              records: [
                record({
                  fields: {
                    ...record().fields,
                    Completed: { type: "INT64", value: 1 },
                  },
                }),
              ],
              continuationMarker: null,
            }
          : {
              records: [
                record({
                  recordName: "Reminder/other",
                  fields: {
                    ...record().fields,
                    List: { type: "REFERENCE", value: { recordName: "List/other" } },
                  },
                }),
                {
                  recordName: "Reminder/deleted",
                  recordType: "Reminder",
                  deleted: true,
                },
                {
                  recordName: "Reminder/softdeleted",
                  recordType: "Reminder",
                  fields: { Deleted: { type: "INT64", value: 1 } },
                },
              ],
              continuationMarker: "query-next",
            },
      );
    });
    const result = await Effect.runPromise(client.readSnapshot());
    expect(result.reminders).toHaveLength(1);
    expect(result.reminders[0]).toMatchObject({ title: "Buy milk", completed: true });
    expect(calls[1].body).toMatchObject({
      zones: [{ syncToken: "lists-next", desiredRecordTypes: ["List"] }],
    });
    expect(calls[3].body).toMatchObject({ continuationMarker: "query-next" });
  });

  it.each(["lists", "query"])("rejects a looping %s cursor", async (kind) => {
    let pages = 0;
    const client = new RemindersCloudKitClient((path) => {
      if (path === "/changes/zone" && kind === "query")
        return Effect.succeed({ zones: [{ records: [list] }] });
      const cursor = ++pages % 2 ? "a" : "b";
      return Effect.succeed(
        kind === "lists"
          ? { zones: [{ records: [], moreComing: true, syncToken: cursor }] }
          : { records: [], continuationMarker: cursor },
      );
    });
    await expect(Effect.runPromise(client.readSnapshot())).rejects.toThrow("protocol");
    expect(pages).toBe(3);
  });

  it("enforces one page budget across discovered lists", async () => {
    let queries = 0;
    const client = new RemindersCloudKitClient((path) => {
      if (path === "/changes/zone")
        return Effect.succeed({
          zones: [
            {
              records: Array.from({ length: 201 }, (_, index) => ({
                ...list,
                recordName: `List/${index}`,
              })),
            },
          ],
        });
      queries++;
      return Effect.succeed({ records: [] });
    });
    await expect(Effect.runPromise(client.readSnapshot())).rejects.toThrow("protocol");
    expect(queries).toBe(200);
  });

  it("enforces one record budget across query pages", async () => {
    let queries = 0;
    const client = new RemindersCloudKitClient((path) => {
      if (path === "/changes/zone")
        return Effect.succeed({ zones: [{ records: [list] }] });
      queries++;
      return Effect.succeed({
        records: Array.from({ length: 5_000 }, (_, index) => ({
          recordName: `Deleted/${index}`,
          deleted: true,
        })),
        continuationMarker: queries === 1 ? "next" : undefined,
      });
    });
    await expect(Effect.runPromise(client.readSnapshot())).rejects.toThrow("protocol");
    expect(queries).toBe(2);
  });

  it("checks later query pages for recurrence before allowing a mutation", async () => {
    const calls: string[] = [];
    const client = new RemindersCloudKitClient((path, body) => {
      calls.push(path);
      if (path === "/records/lookup") return Effect.succeed({ records: [record()] });
      expect(body).toMatchObject({
        query: {
          filterBy: [
            { fieldName: "List", fieldValue: { value: { recordName: "List/1" } } },
            {},
            {},
          ],
        },
      });
      return Effect.succeed(
        (body as { continuationMarker?: string }).continuationMarker
          ? {
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
            }
          : { records: [record()], continuationMarker: "next" },
      );
    });
    const current = await Effect.runPromise(client.getReminder("Reminder/12345678"));
    expect(current?.recurring).toBe(true);
    await expect(
      Effect.runPromise(client.deleteReminder(current as Reminder)),
    ).rejects.toThrow("unsupported");
    expect(calls).toEqual(["/records/lookup", "/records/query", "/records/query"]);
  });

  it.each([
    {},
    { records: [], continuationMarker: 2 },
    { records: [{ recordName: "error", serverErrorCode: "UNKNOWN_ERROR" }] },
    { records: [{ recordName: "RecurrenceRule/1", recordType: "RecurrenceRule" }] },
    {
      records: [
        {
          recordName: "RecurrenceRule/1",
          recordType: "RecurrenceRule",
          fields: { List: { type: "REFERENCE", value: { recordName: "List/1" } } },
        },
      ],
    },
    {
      records: [
        {
          recordName: "RecurrenceRule/1",
          recordType: "RecurrenceRule",
          fields: { Reminder: { type: "REFERENCE", value: { recordName: 1 } } },
        },
      ],
    },
    {
      records: [
        {
          recordName: "RecurrenceRule/1",
          recordType: "RecurrenceRule",
          fields: { Reminder: { type: "STRING", value: "Reminder/12345678" } },
        },
      ],
    },
  ])(
    "rejects uncertain recurrence query responses before mutation: %j",
    async (response) => {
      let failQuery = false;
      const calls: string[] = [];
      const client = new RemindersCloudKitClient((path) => {
        calls.push(path);
        return Effect.succeed(
          path === "/records/lookup"
            ? { records: [record()] }
            : failQuery
              ? response
              : { records: [] },
        );
      });
      const current = await Effect.runPromise(client.getReminder("Reminder/12345678"));
      failQuery = true;
      await expect(
        Effect.runPromise(client.deleteReminder(current as Reminder)),
      ).rejects.toThrow("protocol");
      expect(calls).not.toContain("/records/modify");
    },
  );

  it("ignores soft-deleted reminders with missing content and list references", async () => {
    const client = snapshotClient(() =>
      Effect.succeed({
        zones: [
          {
            records: [
              {
                recordName: "deleted",
                recordType: "Reminder",
                fields: { Deleted: { type: "INT64", value: 1 } },
              },
            ],
          },
        ],
      }),
    );
    expect(await Effect.runPromise(client.readSnapshot())).toEqual({
      lists: [{ id: "List/1", title: "Groceries", color: "blue", count: 1 }],
      reminders: [],
    });
  });
  it.each(["TitleDocument", "NotesDocument"])(
    "keeps encrypted %s unavailable",
    async (field) => {
      const r = record();
      const client = snapshotClient(() =>
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

  it.each(["BYTES", "ENCRYPTED_BYTES"])(
    "decodes server-decrypted %s documents",
    async (type) => {
      const r = record();
      const client = snapshotClient(() =>
        Effect.succeed({
          zones: [
            {
              records: [
                {
                  ...r,
                  fields: {
                    ...r.fields,
                    TitleDocument: { ...r.fields.TitleDocument, type },
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
    },
  );

  it("round trips Unicode and rejects malformed documents", () => {
    expect(decodeCrdtDocument(encodeCrdtDocument("Café 🌿\nMilk"))).toBe(
      "Café 🌿\nMilk",
    );
    expect(() => decodeCrdtDocument("%%%")).toThrow();
  });

  it("reads current reminders with the bounded compound query", async () => {
    const requests: { path: string; body: unknown }[] = [];
    const client = new RemindersCloudKitClient((path, body) => {
      requests.push({ path, body });
      return Effect.succeed(
        path === "/changes/zone"
          ? { zones: [{ records: [list] }] }
          : { records: [record()] },
      );
    });
    const snapshot = await Effect.runPromise(client.readSnapshot());
    expect(requests).toEqual([
      {
        path: "/changes/zone",
        body: {
          zones: [
            {
              zoneID: { zoneName: "Reminders", zoneType: "REGULAR_CUSTOM_ZONE" },
              desiredRecordTypes: ["List"],
            },
          ],
        },
      },
      {
        path: "/records/query",
        body: {
          zoneID: { zoneName: "Reminders", zoneType: "REGULAR_CUSTOM_ZONE" },
          resultsLimit: 50,
          query: {
            recordType: "reminderList",
            filterBy: [
              {
                comparator: "EQUALS",
                fieldName: "List",
                fieldValue: {
                  type: "REFERENCE",
                  value: { recordName: "List/1", action: "VALIDATE" },
                },
              },
              {
                comparator: "EQUALS",
                fieldName: "includeCompleted",
                fieldValue: { type: "INT64", value: 1 },
              },
              {
                comparator: "EQUALS",
                fieldName: "LookupValidatingReference",
                fieldValue: { type: "INT64", value: 1 },
              },
            ],
          },
        },
      },
    ]);
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
          : path === "/records/query"
            ? { records: [] }
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
      "/records/query",
      "/records/lookup",
      "/records/query",
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
          : path === "/records/query"
            ? { records: [] }
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
      "/records/query",
      "/records/lookup",
      "/records/query",
      "/records/modify",
      "/records/lookup",
      "/records/query",
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
          ? {
              records: [
                record({
                  fields: {
                    ...record().fields,
                    RecurrenceRuleIDs: { type: "UNKNOWN_LIST", value: [] },
                  },
                }),
              ],
            }
          : {
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
            },
      );
    });
    const current = await Effect.runPromise(client.getReminder("Reminder/12345678"));
    expect(current?.recurring).toBe(true);
    await expect(
      Effect.runPromise(client.setCompleted(current as Reminder, true)),
    ).rejects.toThrow("unsupported");
    expect(calls).toEqual(["/records/lookup", "/records/query"]);
  });

  it("marks separately referenced recurring reminders in a snapshot", async () => {
    const client = snapshotClient(() =>
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
        path === "/records/lookup" ? { records: [record()] } : { records: [] },
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
    expect(calls).toEqual(["/records/lookup", "/records/query"]);
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

  it.each(["UNKNOWN_LIST", "STRING_LIST"])(
    "updates a reminder with empty %s recurrence IDs and preserves unrelated fields",
    async (recurrenceType) => {
      let modifiedFields: Record<string, unknown> | undefined;
      let written = false;
      const original = record({
        fields: {
          ...record().fields,
          TimeZone: { type: "STRING", value: "America/Vancouver" },
          RecurrenceRuleIDs: { type: recurrenceType, value: [] },
        },
      });
      const client = new RemindersCloudKitClient((path, body) => {
        if (path === "/records/query") return Effect.succeed({ records: [] });
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
      expect(current?.recurring).toBe(false);
      const updated = await Effect.runPromise(
        client.updateReminder(current as Reminder, { flagged: false }),
      );
      expect(updated.flagged).toBe(false);
      expect(modifiedFields).toHaveProperty("Flagged");
      expect(modifiedFields).not.toHaveProperty("TimeZone");
      expect(modifiedFields).not.toHaveProperty("DueDate");
      expect(modifiedFields).not.toHaveProperty("RecurrenceRuleIDs");
    },
  );
});
