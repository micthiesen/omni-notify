import { randomUUID } from "node:crypto";
import { Data, Effect, Schema } from "effect";
import { decodeCrdtDocument, encodeCrdtDocument } from "./codec.js";
import { RemindersCloudKitExtras } from "./cloudkitExtras.js";
import {
  completeRecurringOccurrence,
  type RecurringCompletionTarget,
} from "./recurringCompletion.js";

// CloudKit Reminders record layout adapted from the MIT-licensed iobroker.icloud
// implementation at 07a91933e3f05a36d9c8918ece7f3de295aef805.
const ZONE = { zoneName: "Reminders", zoneType: "REGULAR_CUSTOM_ZONE" } as const;
const MAX_PAGES = 50;
const MAX_LIST_PAGES = 200;
const MAX_QUERY_PAGES = 200;
const MAX_RECORDS = 10_000;
const MAX_FIELD_BYTES = 64 * 1024;

const FieldSchema = Schema.Struct({ type: Schema.String, value: Schema.Unknown });
const RecordSchema = Schema.Struct({
  recordName: Schema.String,
  recordType: Schema.optional(Schema.String),
  recordChangeTag: Schema.optional(Schema.String),
  fields: Schema.optional(Schema.Record(Schema.String, FieldSchema)),
  created: Schema.optional(Schema.Struct({ timestamp: Schema.Number })),
  modified: Schema.optional(Schema.Struct({ timestamp: Schema.Number })),
  deleted: Schema.optional(Schema.Boolean),
  serverErrorCode: Schema.optional(Schema.String),
  reason: Schema.optional(Schema.String),
  errorCode: Schema.optional(Schema.Number),
});
const ErrorSchema = Schema.Struct({
  serverErrorCode: Schema.optional(Schema.String),
  reason: Schema.optional(Schema.String),
  errorCode: Schema.optional(Schema.Number),
});
const ZoneSchema = Schema.Struct({
  records: Schema.optional(Schema.Array(RecordSchema)),
  syncToken: Schema.optional(Schema.String),
  moreComing: Schema.optional(Schema.Boolean),
  error: Schema.optional(ErrorSchema),
});
const ChangesSchema = Schema.Struct({ zones: Schema.Array(ZoneSchema) });
const RecordsSchema = Schema.Struct({ records: Schema.Array(RecordSchema) });
const QuerySchema = Schema.Struct({
  records: Schema.Array(RecordSchema),
  continuationMarker: Schema.optional(Schema.NullOr(Schema.String)),
});

interface ReadBudget {
  pages: number;
  records: number;
}

interface SnapshotIndex {
  lists: Map<string, RemindersListMetadata>;
  records: Map<string, CkRecord>;
  syncToken: string;
}

type CkRecord = Schema.Schema.Type<typeof RecordSchema>;
type CkField = Schema.Schema.Type<typeof FieldSchema>;

export class RemindersError extends Data.TaggedError("RemindersError")<{
  readonly operation: string;
  readonly code:
    | "invalid"
    | "protocol"
    | "conflict"
    | "not_found"
    | "unsupported"
    | "awaiting-device-approval"
    | "uncertain"
    | "transport";
}> {
  public override get message(): string {
    return `Reminders ${this.operation}: ${this.code}`;
  }
}

export interface RemindersList {
  readonly id: string;
  readonly title: string;
  readonly color: string | null;
  /** Incomplete, nondeleted reminders in this list in the complete snapshot. */
  readonly count: number;
  readonly recordChangeTag?: string | null;
}

type RemindersListMetadata = Omit<RemindersList, "count">;

function countedLists(
  lists: Iterable<RemindersListMetadata>,
  reminders: Iterable<Reminder>,
): RemindersList[] {
  const counts = new Map<string, number>();
  for (const reminder of reminders) {
    if (!reminder.completed && !reminder.deleted)
      counts.set(reminder.listId, (counts.get(reminder.listId) ?? 0) + 1);
  }
  return [...lists].map((list) => ({ ...list, count: counts.get(list.id) ?? 0 }));
}

export interface Reminder {
  readonly id: string;
  readonly listId: string;
  readonly title: string;
  readonly description: string;
  readonly completed: boolean;
  readonly completedDate: number | null;
  /** Absolute UTC epoch milliseconds. All-day dates use the caller's civil-date conversion. */
  readonly dueDate: number | null;
  readonly startDate: number | null;
  readonly priority: 0 | 1 | 5 | 9;
  readonly flagged: boolean;
  readonly allDay: boolean;
  readonly deleted: boolean;
  readonly createdDate: number | null;
  readonly lastModifiedDate: number | null;
  readonly recordChangeTag: string;
  readonly recurring: boolean;
}

export interface RemindersSnapshot {
  readonly lists: readonly RemindersList[];
  readonly reminders: readonly Reminder[];
}

export interface ReminderCreateInput {
  /** Stable CloudKit record name supplied by the caller and reused on retry. */
  readonly id: string;
  readonly listId: string;
  readonly title: string;
  readonly description?: string;
  readonly completed?: boolean;
  /** Absolute UTC epoch milliseconds, including for an all-day civil date. */
  readonly dueDate?: number | null;
  readonly startDate?: number | null;
  readonly priority?: 0 | 1 | 5 | 9;
  readonly flagged?: boolean;
  readonly allDay?: boolean;
}

export type ReminderPatch = Partial<
  Pick<
    Reminder,
    | "title"
    | "description"
    | "completed"
    | "dueDate"
    | "startDate"
    | "priority"
    | "flagged"
    | "allDay"
  >
>;

export type CkPost = (
  path: string,
  body: unknown,
) => Effect.Effect<unknown, RemindersError>;

const fail = (operation: string, code: RemindersError["code"]): RemindersError =>
  new RemindersError({ operation, code });

function decode<A, I>(
  schema: Schema.Codec<A, I>,
  value: unknown,
  operation: string,
): Effect.Effect<A, RemindersError> {
  return Schema.decodeUnknownEffect(schema)(value).pipe(
    Effect.mapError(() => fail(operation, "protocol")),
  );
}

function value(fields: Record<string, CkField>, name: string, type: string): unknown {
  const field = fields[name];
  if (!field) return null;
  if (field.type !== type) throw fail(`decode ${name}`, "protocol");
  return field.value;
}

function stringValue(fields: Record<string, CkField>, name: string): string | null {
  const v = value(fields, name, "STRING");
  if (v === null) return null;
  if (typeof v !== "string" || Buffer.byteLength(v) > MAX_FIELD_BYTES * 2) {
    throw fail(`decode ${name}`, "protocol");
  }
  return v;
}

function numberValue(
  fields: Record<string, CkField>,
  name: string,
  type: "INT64" | "TIMESTAMP",
): number | null {
  const v = value(fields, name, type);
  if (v === null) return null;
  if (typeof v !== "number" || !Number.isSafeInteger(v)) {
    throw fail(`decode ${name}`, "protocol");
  }
  return v;
}

function flag(fields: Record<string, CkField>, name: string): boolean {
  const v = numberValue(fields, name, "INT64");
  if (v !== null && v !== 0 && v !== 1) throw fail(`decode ${name}`, "protocol");
  return v === 1;
}

function document(fields: Record<string, CkField>, name: string): string {
  const field = fields[name];
  const raw =
    field?.type === "BYTES" || field?.type === "ENCRYPTED_BYTES"
      ? stringValue({ [name]: { ...field, type: "STRING" } }, name)
      : stringValue(fields, name);
  if (raw === null) return "";
  try {
    return decodeCrdtDocument(raw);
  } catch {
    if (field?.type === "ENCRYPTED_BYTES")
      throw fail(`decode ${name}`, "awaiting-device-approval");
    throw fail(`decode ${name}`, "unsupported");
  }
}

function ref(fields: Record<string, CkField>, name: string): string {
  const v = value(fields, name, "REFERENCE");
  if (
    typeof v !== "object" ||
    v === null ||
    !("recordName" in v) ||
    typeof v.recordName !== "string"
  )
    throw fail(`decode ${name}`, "protocol");
  return v.recordName;
}

function listFromRecord(record: CkRecord): RemindersListMetadata {
  const fields = record.fields ?? {};
  const title = stringValue(fields, "Name");
  if (!title) throw fail("decode list", "protocol");
  return {
    id: record.recordName,
    title,
    color: stringValue(fields, "Color"),
    recordChangeTag: record.recordChangeTag ?? null,
  };
}

function reminderFromRecord(record: CkRecord): Reminder {
  const fields = record.fields ?? {};
  const priority = numberValue(fields, "Priority", "INT64") ?? 0;
  if (priority !== 0 && priority !== 1 && priority !== 5 && priority !== 9) {
    throw fail("decode priority", "protocol");
  }
  const title = document(fields, "TitleDocument");
  if (!title || !record.recordChangeTag) throw fail("decode reminder", "protocol");
  return {
    id: record.recordName,
    listId: ref(fields, "List"),
    title,
    description: document(fields, "NotesDocument"),
    completed: flag(fields, "Completed"),
    completedDate: numberValue(fields, "CompletionDate", "TIMESTAMP"),
    dueDate: numberValue(fields, "DueDate", "TIMESTAMP"),
    startDate: numberValue(fields, "StartDate", "TIMESTAMP"),
    priority,
    flagged: flag(fields, "Flagged"),
    allDay: flag(fields, "AllDay"),
    deleted: flag(fields, "Deleted"),
    createdDate:
      numberValue(fields, "CreationDate", "TIMESTAMP") ??
      record.created?.timestamp ??
      null,
    lastModifiedDate:
      numberValue(fields, "LastModifiedDate", "TIMESTAMP") ??
      record.modified?.timestamp ??
      null,
    recordChangeTag: record.recordChangeTag,
    recurring: Object.entries(fields).some(([key, field]) => {
      if (!/recurr|repeat/i.test(key)) return false;
      return !(
        key === "RecurrenceRuleIDs" &&
        (field.type === "UNKNOWN_LIST" || field.type === "STRING_LIST") &&
        Array.isArray(field.value) &&
        field.value.length === 0
      );
    }),
  };
}

function recurrenceReferences(record: CkRecord): string[] {
  if (
    record.deleted ||
    (record.fields?.Deleted?.type === "INT64" && record.fields.Deleted.value === 1) ||
    !/recurr|repeat/i.test(record.recordType ?? "")
  )
    return [];
  const reminderId = ref(record.fields ?? {}, "Reminder");
  if (!reminderId) throw fail("recurrence reference", "protocol");
  const ids: string[] = [];
  for (const field of Object.values(record.fields ?? {})) {
    if (field.type !== "REFERENCE") continue;
    if (
      typeof field.value === "object" &&
      field.value !== null &&
      "recordName" in field.value &&
      typeof field.value.recordName === "string" &&
      field.value.recordName.length > 0
    )
      ids.push(field.value.recordName);
    else throw fail("recurrence reference", "protocol");
  }
  if (!ids.length) throw fail("recurrence reference", "protocol");
  return ids;
}

function validateText(text: string, operation: string): void {
  if (!text.trim() || Buffer.byteLength(text, "utf8") > MAX_FIELD_BYTES) {
    throw fail(operation, "invalid");
  }
}

function validateDate(date: number | null | undefined, operation: string): void {
  if (
    date !== undefined &&
    date !== null &&
    (!Number.isSafeInteger(date) || date < 0)
  ) {
    throw fail(operation, "invalid");
  }
}

const isAllDayDate = (date: number | null | undefined): boolean =>
  date === undefined || date === null || date % 86_400_000 === 0;

function tokenMap(names: readonly string[], now: number, replica: string): string {
  const map: Record<string, unknown> = {};
  for (const name of names) {
    map[name] = {
      counter: 1,
      modificationTime: now / 1000 - 978_307_200,
      replicaID: replica,
    };
  }
  return JSON.stringify({ map });
}

const int = (value: number | null) => ({ type: "INT64", value });
const stamp = (value: number | null) => ({ type: "TIMESTAMP", value });
const str = (value: string) => ({ type: "STRING", value });

function patchFields(patch: ReminderPatch, now: number, replica: string) {
  const fields: Record<string, unknown> = {};
  const names: string[] = [];
  const put = (name: string, entry: unknown) => {
    fields[name] = entry;
    names.push(name[0].toLowerCase() + name.slice(1));
  };
  if (patch.title !== undefined) {
    validateText(patch.title, "title");
    put("TitleDocument", str(encodeCrdtDocument(patch.title)));
  }
  if (patch.description !== undefined) {
    if (Buffer.byteLength(patch.description, "utf8") > MAX_FIELD_BYTES)
      throw fail("notes", "invalid");
    put("NotesDocument", str(encodeCrdtDocument(patch.description)));
  }
  if (patch.completed !== undefined) {
    put("Completed", int(patch.completed ? 1 : 0));
    put("CompletionDate", stamp(patch.completed ? now : null));
  }
  if (patch.dueDate !== undefined) {
    validateDate(patch.dueDate, "due date");
    put("DueDate", stamp(patch.dueDate));
  }
  if (patch.startDate !== undefined) {
    validateDate(patch.startDate, "start date");
    put("StartDate", stamp(patch.startDate));
  }
  if (patch.priority !== undefined) {
    if (![0, 1, 5, 9].includes(patch.priority)) throw fail("priority", "invalid");
    put("Priority", int(patch.priority));
  }
  if (patch.flagged !== undefined) put("Flagged", int(patch.flagged ? 1 : 0));
  if (patch.allDay !== undefined) put("AllDay", int(patch.allDay ? 1 : 0));
  if (!names.length) throw fail("update", "invalid");
  put("LastModifiedDate", stamp(now));
  fields.ResolutionTokenMap = str(tokenMap(names, now, replica));
  return fields;
}

export class RemindersCloudKitClient {
  private listCache?: { lists: Map<string, RemindersListMetadata>; syncToken: string };
  private snapshotIndex?: SnapshotIndex;
  public readonly extras: RemindersCloudKitExtras;
  public constructor(private readonly ckPost: CkPost) {
    this.extras = new RemindersCloudKitExtras(ckPost, (listId) =>
      this.queryList(listId, { pages: 0, records: 0 }),
    );
  }

  public hasSnapshot(): boolean {
    return this.snapshotIndex !== undefined;
  }

  public completeRecurring(target: RecurringCompletionTarget) {
    return completeRecurringOccurrence(
      {
        post: this.ckPost,
        getReminder: (id) => this.getReminder(id),
        getRecurrences: (id) => this.extras.getRecurrences(id),
      },
      target,
    );
  }

  public invalidateSnapshot(): Effect.Effect<void> {
    return Effect.sync(() => {
      this.snapshotIndex = undefined;
      this.listCache = undefined;
    });
  }

  private post<A, I>(path: string, body: unknown, schema: Schema.Codec<A, I>) {
    return this.ckPost(path, body).pipe(
      Effect.flatMap((raw) => {
        if (typeof raw !== "object" || raw === null) {
          return Effect.fail(fail(path, "protocol"));
        }
        const arrays = [
          "zones" in raw ? raw.zones : undefined,
          "records" in raw ? raw.records : undefined,
        ];
        if (
          arrays.some((array) => Array.isArray(array) && array.length > MAX_RECORDS)
        ) {
          return Effect.fail(fail(path, "protocol"));
        }
        return decode(schema, raw, path);
      }),
    );
  }

  public readSnapshot(): Effect.Effect<RemindersSnapshot, RemindersError> {
    return Effect.suspend(() =>
      this.snapshotIndex
        ? this.refreshSnapshot(this.snapshotIndex)
        : this.readFullSnapshot(),
    );
  }

  private readFullSnapshot(): Effect.Effect<RemindersSnapshot, RemindersError> {
    return Effect.gen({ self: this }, function* () {
      const lists = new Map(this.listCache?.lists);
      const reminders = new Map<string, Reminder>();
      const recurringIds = new Set<string>();
      const indexedRecords = new Map<string, CkRecord>();
      let token = this.listCache?.syncToken;
      let latestToken: string | undefined;
      let complete = false;
      const budget: ReadBudget = { pages: 0, records: 0 };
      const cursors = new Set<string>(token ? [token] : []);
      for (let page = 0; page < MAX_LIST_PAGES; page++) {
        const response = yield* this.post(
          "/changes/zone",
          {
            zones: [
              {
                zoneID: ZONE,
                desiredRecordTypes: ["List"],
                ...(token ? { syncToken: token } : {}),
              },
            ],
          },
          ChangesSchema,
        );
        if (response.zones.length !== 1)
          return yield* Effect.fail(fail("snapshot zones", "protocol"));
        const zone = response.zones[0];
        if (zone.error) return yield* Effect.fail(fail("snapshot zone", "protocol"));
        for (const record of zone.records ?? []) {
          if (++budget.records > MAX_RECORDS)
            return yield* Effect.fail(fail("snapshot limit", "protocol"));
          if (record.serverErrorCode || record.errorCode) {
            return yield* Effect.fail(fail("snapshot record", "protocol"));
          }
          if (
            record.deleted ||
            (record.fields?.Deleted?.type === "INT64" &&
              record.fields.Deleted.value === 1)
          )
            lists.delete(record.recordName);
          else if (record.recordType === "List") {
            lists.set(
              record.recordName,
              yield* Effect.try({
                try: () => listFromRecord(record),
                catch: () => fail("decode list", "protocol"),
              }),
            );
          }
        }
        latestToken = zone.syncToken;
        if (!zone.moreComing) {
          complete = true;
          break;
        }
        if (!zone.syncToken || cursors.has(zone.syncToken)) {
          return yield* Effect.fail(fail("snapshot cursor", "protocol"));
        }
        token = zone.syncToken;
        cursors.add(token);
      }
      if (!complete) return yield* Effect.fail(fail("snapshot limit", "protocol"));
      if (latestToken)
        this.listCache = { lists: new Map(lists), syncToken: latestToken };
      for (const listId of lists.keys()) {
        const records = yield* this.queryList(listId, budget);
        for (const record of records) {
          if (
            record.deleted ||
            (record.fields?.Deleted?.type === "INT64" &&
              record.fields.Deleted.value === 1)
          )
            continue;
          if (record.recordType === "Reminder") {
            const reminder = yield* Effect.try({
              try: () => reminderFromRecord(record),
              catch: (cause) =>
                cause instanceof RemindersError
                  ? cause
                  : fail("decode reminder", "protocol"),
            });
            if (reminder.listId === listId && !reminder.deleted) {
              reminders.set(reminder.id, reminder);
              indexedRecords.set(record.recordName, record);
            }
          } else {
            const references = yield* Effect.try({
              try: () => recurrenceReferences(record),
              catch: () => fail("recurrence reference", "protocol"),
            });
            for (const id of references) recurringIds.add(id);
            if (references.length) indexedRecords.set(record.recordName, record);
          }
        }
      }
      if (lists.size + indexedRecords.size > MAX_RECORDS)
        return yield* Effect.fail(fail("snapshot index limit", "protocol"));
      this.snapshotIndex = latestToken
        ? { lists: new Map(lists), records: indexedRecords, syncToken: latestToken }
        : undefined;
      return {
        lists: countedLists(lists.values(), reminders.values()),
        reminders: [...reminders.values()].map((reminder) =>
          recurringIds.has(reminder.id) ? { ...reminder, recurring: true } : reminder,
        ),
      };
    });
  }

  private refreshSnapshot(
    index: SnapshotIndex,
  ): Effect.Effect<RemindersSnapshot, RemindersError> {
    return Effect.gen({ self: this }, function* () {
      const lists = new Map(index.lists);
      const records = new Map(index.records);
      let token = index.syncToken;
      let seen = 0;
      const cursors = new Set([token]);
      for (let page = 0; page < MAX_LIST_PAGES; page++) {
        const response = yield* this.post(
          "/changes/zone",
          {
            zones: [{ zoneID: ZONE, syncToken: token }],
          },
          ChangesSchema,
        );
        if (
          response.zones.length !== 1 ||
          response.zones[0].error ||
          !response.zones[0].records
        )
          return yield* Effect.fail(fail("snapshot refresh", "protocol"));
        const zone = response.zones[0];
        for (const record of zone.records!) {
          if (++seen > MAX_RECORDS || record.serverErrorCode || record.errorCode)
            return yield* Effect.fail(fail("snapshot refresh", "protocol"));
          if (
            record.deleted ||
            (record.fields?.Deleted?.type === "INT64" &&
              record.fields.Deleted.value === 1)
          ) {
            lists.delete(record.recordName);
            records.delete(record.recordName);
          } else if (record.recordType === "List") {
            const list = yield* Effect.try({
              try: () => listFromRecord(record),
              catch: () => fail("decode list", "protocol"),
            });
            lists.set(record.recordName, list);
          } else if (
            record.recordType === "Reminder" ||
            /recurr|repeat/i.test(record.recordType ?? "")
          ) {
            records.set(record.recordName, record);
          } else if (!record.recordType) {
            return yield* Effect.fail(fail("snapshot refresh record", "protocol"));
          }
          if (lists.size + records.size > MAX_RECORDS)
            return yield* Effect.fail(fail("snapshot index limit", "protocol"));
        }
        if (!zone.moreComing) {
          if (!zone.syncToken)
            return yield* Effect.fail(fail("snapshot refresh cursor", "protocol"));
          const reminders = new Map<string, Reminder>();
          const recurringIds = new Set<string>();
          for (const record of records.values()) {
            if (record.recordType === "Reminder") {
              const reminder = yield* Effect.try({
                try: () => reminderFromRecord(record),
                catch: (cause) =>
                  cause instanceof RemindersError
                    ? cause
                    : fail("decode reminder", "protocol"),
              });
              if (lists.has(reminder.listId)) reminders.set(reminder.id, reminder);
              else records.delete(record.recordName);
            } else {
              const references = yield* Effect.try({
                try: () => recurrenceReferences(record),
                catch: () => fail("recurrence reference", "protocol"),
              });
              for (const id of references) recurringIds.add(id);
            }
          }
          this.snapshotIndex = { lists, records, syncToken: zone.syncToken };
          this.listCache = { lists: new Map(lists), syncToken: zone.syncToken };
          return {
            lists: countedLists(lists.values(), reminders.values()),
            reminders: [...reminders.values()].map((reminder) =>
              recurringIds.has(reminder.id)
                ? { ...reminder, recurring: true }
                : reminder,
            ),
          };
        }
        if (!zone.syncToken || cursors.has(zone.syncToken))
          return yield* Effect.fail(fail("snapshot refresh cursor", "protocol"));
        token = zone.syncToken;
        cursors.add(token);
      }
      return yield* Effect.fail(fail("snapshot refresh limit", "protocol"));
    });
  }

  public verifyReadAccess(): Effect.Effect<void, RemindersError> {
    return Effect.gen({ self: this }, function* () {
      let token: string | undefined;
      let seen = 0;
      const cursors = new Set<string>();
      for (let page = 0; page < MAX_PAGES; page++) {
        const response = yield* this.post(
          "/changes/zone",
          {
            zones: [
              {
                zoneID: ZONE,
                reverse: true,
                desiredRecordTypes: ["List", "Reminder"],
                ...(token ? { syncToken: token } : {}),
              },
            ],
          },
          ChangesSchema,
        );
        if (response.zones.length !== 1 || response.zones[0].error)
          return yield* Effect.fail(fail("access probe", "protocol"));
        const zone = response.zones[0];
        if (!zone.records) return yield* Effect.fail(fail("access probe", "protocol"));
        let decodedReminder = false;
        for (const record of zone.records) {
          if (++seen > MAX_RECORDS || record.serverErrorCode || record.errorCode)
            return yield* Effect.fail(fail("access probe", "protocol"));
          if (
            record.deleted ||
            (record.fields?.Deleted?.type === "INT64" &&
              record.fields.Deleted.value === 1)
          )
            continue;
          if (record.recordType === "Reminder") {
            yield* Effect.try({
              try: () => reminderFromRecord(record),
              catch: (cause) =>
                cause instanceof RemindersError
                  ? cause
                  : fail("access probe", "protocol"),
            });
            decodedReminder = true;
          } else if (record.recordType === "List") {
            yield* Effect.try({
              try: () => listFromRecord(record),
              catch: () => fail("access probe", "protocol"),
            });
          } else {
            return yield* Effect.fail(fail("access probe", "protocol"));
          }
        }
        if (decodedReminder || !zone.moreComing) return;
        if (!zone.syncToken || cursors.has(zone.syncToken))
          return yield* Effect.fail(fail("access probe cursor", "protocol"));
        token = zone.syncToken;
        cursors.add(token);
      }
      return yield* Effect.fail(fail("access probe limit", "protocol"));
    });
  }

  private lookup(id: string): Effect.Effect<CkRecord | null, RemindersError> {
    return Effect.gen({ self: this }, function* () {
      const result = yield* this.post(
        "/records/lookup",
        { records: [{ recordName: id }], zoneID: ZONE },
        RecordsSchema,
      );
      if (result.records.length !== 1 || result.records[0].recordName !== id) {
        return yield* Effect.fail(fail("lookup", "protocol"));
      }
      const record = result.records[0];
      if (
        record.serverErrorCode === "NOT_FOUND" ||
        record.serverErrorCode === "UNKNOWN_ITEM"
      )
        return null;
      if (record.serverErrorCode || record.errorCode)
        return yield* Effect.fail(fail("lookup", "protocol"));
      return record.deleted ||
        (record.fields?.Deleted?.type === "INT64" && record.fields.Deleted.value === 1)
        ? null
        : record;
    });
  }

  private queryList(
    listId: string,
    budget: ReadBudget,
  ): Effect.Effect<CkRecord[], RemindersError> {
    return Effect.gen({ self: this }, function* () {
      let continuationMarker: string | undefined;
      const cursors = new Set<string>();
      const records: CkRecord[] = [];
      while (budget.pages++ < MAX_QUERY_PAGES) {
        const response = yield* this.post(
          "/records/query",
          {
            zoneID: ZONE,
            resultsLimit: 50,
            query: {
              recordType: "reminderList",
              filterBy: [
                {
                  comparator: "EQUALS",
                  fieldName: "List",
                  fieldValue: {
                    type: "REFERENCE",
                    value: { recordName: listId, action: "VALIDATE" },
                  },
                },
                {
                  comparator: "EQUALS",
                  fieldName: "includeCompleted",
                  fieldValue: int(1),
                },
                {
                  comparator: "EQUALS",
                  fieldName: "LookupValidatingReference",
                  fieldValue: int(1),
                },
              ],
            },
            ...(continuationMarker ? { continuationMarker } : {}),
          },
          QuerySchema,
        );
        for (const record of response.records) {
          if (
            ++budget.records > MAX_RECORDS ||
            record.serverErrorCode ||
            record.errorCode
          ) {
            return yield* Effect.fail(fail("list query", "protocol"));
          }
          records.push(record);
        }
        if (!response.continuationMarker) return records;
        if (cursors.has(response.continuationMarker)) {
          return yield* Effect.fail(fail("list query cursor", "protocol"));
        }
        continuationMarker = response.continuationMarker;
        cursors.add(continuationMarker);
      }
      return yield* Effect.fail(fail("list query limit", "protocol"));
    });
  }

  private hasRecurrenceReference(
    id: string,
    listId: string,
  ): Effect.Effect<boolean, RemindersError> {
    return Effect.gen({ self: this }, function* () {
      if (this.snapshotIndex) {
        const snapshot = yield* this.readSnapshot();
        const reminder = snapshot.reminders.find((reminder) => reminder.id === id);
        if (!reminder)
          return yield* Effect.fail(fail("recurrence snapshot", "protocol"));
        return reminder.recurring;
      }
      const records = yield* this.queryList(listId, { pages: 0, records: 0 });
      return yield* Effect.try({
        try: () => records.some((record) => recurrenceReferences(record).includes(id)),
        catch: () => fail("recurrence reference", "protocol"),
      });
    });
  }

  public getReminder(id: string): Effect.Effect<Reminder | null, RemindersError> {
    return Effect.gen({ self: this }, function* () {
      const record = yield* this.lookup(id);
      if (!record) return null;
      if (record.recordType !== "Reminder")
        return yield* Effect.fail(fail("get", "protocol"));
      const reminder = yield* Effect.try({
        try: () => reminderFromRecord(record),
        catch: (cause) =>
          cause instanceof RemindersError ? cause : fail("get", "protocol"),
      });
      if (reminder.deleted) return null;
      if (reminder.recurring) return reminder;
      const recurring = yield* this.hasRecurrenceReference(id, reminder.listId);
      return recurring ? { ...reminder, recurring: true } : reminder;
    });
  }

  private modify(operationType: "create" | "update", record: object, id: string) {
    return Effect.gen({ self: this }, function* () {
      const result = yield* this.post(
        "/records/modify",
        { operations: [{ operationType, record }], zoneID: ZONE, atomic: true },
        RecordsSchema,
      );
      if (
        result.records.length === 1 &&
        result.records[0].recordName === id &&
        ["CONFLICT", "SERVER_RECORD_CHANGED"].includes(
          result.records[0].serverErrorCode ?? "",
        )
      ) {
        return yield* Effect.fail(fail("modify", "conflict"));
      }
      if (
        result.records.length !== 1 ||
        result.records[0].recordName !== id ||
        result.records[0].serverErrorCode ||
        result.records[0].errorCode ||
        !result.records[0].recordChangeTag
      ) {
        return yield* Effect.fail(fail("modify", "protocol"));
      }
      return result.records[0].recordChangeTag;
    });
  }

  public createReminder(
    input: ReminderCreateInput,
  ): Effect.Effect<Reminder, RemindersError> {
    return Effect.gen({ self: this }, function* () {
      if (
        !/^Reminder\/[A-Za-z0-9-]{8,128}$/.test(input.id) ||
        !input.listId ||
        input.listId.length > 256
      ) {
        return yield* Effect.fail(fail("create", "invalid"));
      }
      if (input.allDay && !isAllDayDate(input.dueDate))
        return yield* Effect.fail(fail("all-day due date", "invalid"));
      const prior = yield* this.getReminder(input.id);
      const expected = {
        listId: input.listId,
        title: input.title,
        description: input.description ?? "",
        completed: input.completed ?? false,
        dueDate: input.dueDate ?? null,
        startDate: input.startDate ?? null,
        priority: input.priority ?? 0,
        flagged: input.flagged ?? false,
        allDay: input.allDay ?? false,
      };
      const matches = (reminder: Reminder) =>
        Object.entries(expected).every(
          ([key, value]) => reminder[key as keyof Reminder] === value,
        );
      if (prior) {
        if (matches(prior)) return prior;
        return yield* Effect.fail(fail("create", "conflict"));
      }
      const now = Date.now();
      const fields = yield* Effect.try({
        try: () =>
          patchFields(
            {
              title: input.title,
              description: input.description ?? "",
              completed: input.completed ?? false,
              dueDate: input.dueDate,
              startDate: input.startDate,
              priority: input.priority ?? 0,
              flagged: input.flagged ?? false,
              allDay: input.allDay ?? false,
            },
            now,
            randomUUID().toUpperCase(),
          ),
        catch: (cause) =>
          cause instanceof RemindersError ? cause : fail("create", "invalid"),
      });
      fields.CreationDate = stamp(now);
      fields.Deleted = int(0);
      fields.Imported = int(0);
      fields.List = {
        type: "REFERENCE",
        value: { recordName: input.listId, action: "VALIDATE" },
      };
      const tag = yield* this.modify(
        "create",
        {
          recordName: input.id,
          recordType: "Reminder",
          fields,
          createShortGUID: true,
        },
        input.id,
      );
      const written = yield* this.getReminder(input.id);
      if (!written || written.recordChangeTag !== tag || !matches(written))
        return yield* Effect.fail(fail("create verification", "protocol"));
      return written;
    });
  }

  public updateReminder(
    current: Reminder,
    patch: ReminderPatch,
  ): Effect.Effect<Reminder, RemindersError> {
    return Effect.gen({ self: this }, function* () {
      if (current.recurring)
        return yield* Effect.fail(fail("update recurring", "unsupported"));
      if (!current.recordChangeTag)
        return yield* Effect.fail(fail("update", "invalid"));
      const latest = yield* this.getReminder(current.id);
      if (!latest) return yield* Effect.fail(fail("update", "not_found"));
      if (latest.recurring)
        return yield* Effect.fail(fail("update recurring", "unsupported"));
      if (latest.recordChangeTag !== current.recordChangeTag)
        return yield* Effect.fail(fail("update", "conflict"));
      if (
        (patch.dueDate !== undefined || patch.allDay !== undefined) &&
        (patch.allDay ?? latest.allDay) &&
        !isAllDayDate(patch.dueDate === undefined ? latest.dueDate : patch.dueDate)
      )
        return yield* Effect.fail(fail("all-day due date", "invalid"));
      const fields = yield* Effect.try({
        try: () => patchFields(patch, Date.now(), randomUUID().toUpperCase()),
        catch: (cause) =>
          cause instanceof RemindersError ? cause : fail("update", "invalid"),
      });
      const tag = yield* this.modify(
        "update",
        {
          recordName: current.id,
          recordType: "Reminder",
          recordChangeTag: current.recordChangeTag,
          fields,
        },
        current.id,
      );
      const written = yield* this.getReminder(current.id);
      if (
        !written ||
        written.recordChangeTag !== tag ||
        Object.entries(patch).some(
          ([key, expected]) => written[key as keyof Reminder] !== expected,
        )
      ) {
        return yield* Effect.fail(fail("update verification", "protocol"));
      }
      return written;
    });
  }

  public setCompleted(current: Reminder, completed: boolean) {
    return this.updateReminder(current, { completed });
  }

  public deleteReminder(current: Reminder): Effect.Effect<void, RemindersError> {
    return Effect.gen({ self: this }, function* () {
      if (current.recurring)
        return yield* Effect.fail(fail("delete recurring", "unsupported"));
      if (!current.recordChangeTag)
        return yield* Effect.fail(fail("delete", "invalid"));
      const latest = yield* this.getReminder(current.id);
      if (!latest) return yield* Effect.fail(fail("delete", "not_found"));
      if (latest.recurring)
        return yield* Effect.fail(fail("delete recurring", "unsupported"));
      if (latest.recordChangeTag !== current.recordChangeTag)
        return yield* Effect.fail(fail("delete", "conflict"));
      const now = Date.now();
      yield* this.modify(
        "update",
        {
          recordName: current.id,
          recordType: "Reminder",
          recordChangeTag: current.recordChangeTag,
          fields: {
            Deleted: int(1),
            LastModifiedDate: stamp(now),
            ResolutionTokenMap: str(
              tokenMap(
                ["deleted", "lastModifiedDate"],
                now,
                randomUUID().toUpperCase(),
              ),
            ),
          },
        },
        current.id,
      );
      if (yield* this.getReminder(current.id))
        return yield* Effect.fail(fail("delete verification", "protocol"));
    });
  }
}
