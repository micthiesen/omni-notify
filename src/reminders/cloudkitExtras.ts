import { randomUUID } from "node:crypto";
import { Clock, Effect, Result, Schema } from "effect";
import { RemindersError, type CkPost } from "./cloudkit.js";
import {
  decodeRecurrenceValues,
  encodeRecurrenceValues,
  type RecurrenceDecoded,
  type RecurrenceRule,
} from "./recurrence.js";

const zoneID = { zoneName: "Reminders", zoneType: "REGULAR_CUSTOM_ZONE" };
const Field = Schema.Struct({ type: Schema.String, value: Schema.Unknown });
const RecordSchema = Schema.Struct({
  recordName: Schema.String,
  recordType: Schema.optional(Schema.String),
  recordChangeTag: Schema.optional(Schema.String),
  fields: Schema.optional(Schema.Record(Schema.String, Field)),
  deleted: Schema.optional(Schema.Boolean),
  serverErrorCode: Schema.optional(Schema.String),
  errorCode: Schema.optional(Schema.Number),
});
const Records = Schema.Struct({ records: Schema.Array(RecordSchema) });
type RecordValue = Schema.Schema.Type<typeof RecordSchema>;
type Fields = Record<string, Schema.Schema.Type<typeof Field>>;
export type RecurrenceInput = RecurrenceRule;
type WriteFields = Record<string, { type?: string; value: unknown }>;
export interface ReminderRecurrence {
  readonly id: string;
  readonly reminderId: string;
  readonly recordChangeTag: string | null;
  readonly writable: boolean;
  readonly recurrence: RecurrenceDecoded;
}
export type ReminderRecurrences = {
  readonly reminderId: string;
  readonly reminderChangeTag: string;
  readonly rules: readonly ReminderRecurrence[];
};
export interface ReminderListDetails {
  readonly id: string;
  readonly title: string;
  readonly color: string | null;
  readonly recordChangeTag: string | null;
}
const fail = (operation: string, code: RemindersError["code"] = "protocol") =>
  new RemindersError({ operation, code });
const scalar = new Set([
  "Frequency",
  "Interval",
  "OccurrenceCount",
  "FirstDayOfTheWeek",
]);
const selectors = new Set([
  "DaysOfTheWeek",
  "DaysOfTheMonth",
  "DaysOfTheYear",
  "WeeksOfTheYear",
  "MonthsOfTheYear",
  "SetPositions",
]);
const admin = new Set(["Reminder", "Deleted", "Imported", "ResolutionTokenMap"]);
const gone = (record: RecordValue) =>
  record.deleted ||
  (record.fields?.Deleted?.type === "INT64" && record.fields.Deleted.value === 1);
const int = (value: number | null) => ({ type: "INT64", value });
const str = (value: string) => ({ type: "STRING", value });
const reference = (recordName: string) => ({
  type: "REFERENCE",
  value: { recordName, action: "VALIDATE" },
});
const refName = (field: Fields[string] | undefined): string | null => {
  const parsed = Schema.decodeUnknownResult(
    Schema.Struct({
      type: Schema.Literal("REFERENCE"),
      value: Schema.Struct({ recordName: Schema.String }),
    }),
  )(field);
  return Result.isSuccess(parsed) ? parsed.success.value.recordName : null;
};
function tokenMap(record: RecordValue, names: readonly string[], now: number): string {
  const field = record.fields?.ResolutionTokenMap;
  let current: Record<string, unknown> = { map: {} };
  if (field) {
    if (
      field.type !== "STRING" ||
      typeof field.value !== "string" ||
      field.value.length > 65_536
    )
      throw fail("resolution tokens");
    const parsed = Schema.decodeUnknownResult(
      Schema.Record(Schema.String, Schema.Unknown),
    )(JSON.parse(field.value));
    if (Result.isFailure(parsed)) throw fail("resolution tokens");
    current = parsed.success;
  }
  const map = Schema.decodeUnknownResult(Schema.Record(Schema.String, Schema.Unknown))(
    current.map,
  );
  if (Result.isFailure(map)) throw fail("resolution tokens");
  const next = { ...map.success };
  for (const name of names) {
    const prior = next[name];
    const parsed = Schema.decodeUnknownResult(
      Schema.Struct({ counter: Schema.Number }),
    )(prior);
    if (prior !== undefined && Result.isFailure(parsed))
      throw fail("resolution tokens");
    const counter = Result.isSuccess(parsed) ? parsed.success.counter : 0;
    if (
      !Number.isSafeInteger(counter) ||
      counter < 0 ||
      counter >= Number.MAX_SAFE_INTEGER
    )
      throw fail("resolution tokens");
    next[name] = {
      counter: counter + 1,
      modificationTime: now / 1000 - 978_307_200,
      replicaID: randomUUID().toUpperCase(),
    };
  }
  return JSON.stringify({ ...current, map: next });
}
function linkedIds(record: RecordValue): string[] {
  const field = record.fields?.RecurrenceRuleIDs;
  if (!field) return [];
  if (field.type !== "STRING_LIST" && field.type !== "UNKNOWN_LIST")
    throw fail("recurrence IDs", "unsupported");
  const ids = Schema.decodeUnknownResult(Schema.Array(Schema.String))(field.value);
  if (
    Result.isFailure(ids) ||
    ids.success.length > 100 ||
    (field.type === "UNKNOWN_LIST" && ids.success.length > 0)
  )
    throw fail("recurrence IDs", "unsupported");
  if (
    ids.success.some((id) => !/^[A-Za-z0-9-]{8,128}$/.test(id)) ||
    new Set(ids.success).size !== ids.success.length
  )
    throw fail("recurrence IDs", "unsupported");
  return [...ids.success];
}
function ruleDetails(record: RecordValue, reminderId: string): ReminderRecurrence {
  if (
    record.recordType !== "RecurrenceRule" ||
    refName(record.fields?.Reminder) !== reminderId
  )
    throw fail("recurrence owner");
  const fields = record.fields ?? {};
  const values: Record<string, unknown> = {};
  let validTypes = true;
  for (const [name, field] of Object.entries(fields)) {
    if (admin.has(name)) continue;
    values[name] = field.value;
    if (scalar.has(name) && field.type !== "INT64") validTypes = false;
    if (selectors.has(name) && !["STRING", "BYTES"].includes(field.type))
      validTypes = false;
    if (name === "EndDate" && !["TIMESTAMP", "INT64", "DOUBLE"].includes(field.type))
      validTypes = false;
  }
  const recurrence: RecurrenceDecoded = validTypes
    ? decodeRecurrenceValues(values)
    : { supported: false, reason: "invalid_fields", fields: [] };
  return {
    id: record.recordName,
    reminderId,
    recordChangeTag: record.recordChangeTag ?? null,
    recurrence,
    writable: !!record.recordChangeTag && recurrence.supported,
  };
}
function recurrenceFields(input: RecurrenceInput, existing: Fields = {}): WriteFields {
  const encoded = encodeRecurrenceValues(input);
  if (!encoded.supported) throw fail("recurrence fields", "unsupported");
  const fields: WriteFields = {};
  for (const [name, value] of Object.entries(encoded.values)) {
    // Apple CKModelProperty writes service values without guessing a CloudKit type.
    // The Web Services field dictionary's type is optional. Preserve a type read
    // from the exact existing field; scalar INT64 types are independently proven.
    // https://developer.apple.com/library/archive/documentation/DataManagement/Conceptual/CloudKitWebServicesReference/Types.html
    fields[name] = scalar.has(name)
      ? { type: "INT64", value }
      : existing[name]
        ? { type: existing[name].type, value }
        : { value };
  }
  return fields;
}

/** Extra bounded operations; all writes use CAS and exact multi-record acknowledgment. */
export class RemindersCloudKitExtras {
  constructor(
    private readonly post: CkPost,
    private readonly queryList: (
      listId: string,
    ) => Effect.Effect<readonly unknown[], RemindersError>,
  ) {}
  private request(path: string, body: unknown) {
    return this.post(path, body).pipe(
      Effect.flatMap((response) => Schema.decodeUnknownEffect(Records)(response)),
      Effect.mapError((cause) =>
        cause instanceof RemindersError ? cause : fail("records"),
      ),
    );
  }
  private lookup(id: string) {
    return Effect.gen({ self: this }, function* () {
      const response = yield* this.request("/records/lookup", {
        zoneID,
        records: [{ recordName: id }],
      });
      if (response.records.length !== 1 || response.records[0].recordName !== id)
        return yield* Effect.fail(fail("lookup"));
      const record = response.records[0];
      if (
        ["NOT_FOUND", "UNKNOWN_ITEM"].includes(record.serverErrorCode ?? "") ||
        gone(record)
      )
        return null;
      if (record.serverErrorCode || record.errorCode)
        return yield* Effect.fail(fail("lookup"));
      return record;
    });
  }
  private require(id: string, type: string, tag?: string) {
    return this.lookup(id).pipe(
      Effect.flatMap((record) => {
        if (!record) return Effect.fail(fail("record", "not_found"));
        if (record.recordType !== type || !record.recordChangeTag)
          return Effect.fail(fail("record"));
        if (tag !== undefined && record.recordChangeTag !== tag)
          return Effect.fail(fail("record", "conflict"));
        return Effect.succeed(record);
      }),
    );
  }
  private modify(
    operations: readonly {
      operationType: "create" | "update";
      record: { recordName: string; [key: string]: unknown };
    }[],
  ) {
    return Effect.gen({ self: this }, function* () {
      const response = yield* this.request("/records/modify", {
        zoneID,
        atomic: true,
        operations,
      });
      const expected = new Set(operations.map((op) => op.record.recordName));
      const tags = new Map<string, string>();
      if (response.records.length !== expected.size)
        return yield* Effect.fail(fail("modify acknowledgments"));
      for (const record of response.records) {
        if (!expected.has(record.recordName) || tags.has(record.recordName))
          return yield* Effect.fail(fail("modify acknowledgments"));
        if (
          ["CONFLICT", "SERVER_RECORD_CHANGED"].includes(record.serverErrorCode ?? "")
        )
          return yield* Effect.fail(fail("modify", "conflict"));
        if (record.serverErrorCode || record.errorCode || !record.recordChangeTag)
          return yield* Effect.fail(fail("modify acknowledgments"));
        tags.set(record.recordName, record.recordChangeTag);
      }
      return tags;
    });
  }
  getList(id: string): Effect.Effect<ReminderListDetails | null, RemindersError> {
    return Effect.gen({ self: this }, function* () {
      const record = yield* this.lookup(id);
      if (!record) return null;
      if (record.recordType !== "List") return yield* Effect.fail(fail("list"));
      const fields = record.fields ?? {};
      if (
        fields.Name?.type !== "STRING" ||
        typeof fields.Name.value !== "string" ||
        fields.Name.value.length > 4096
      )
        return yield* Effect.fail(fail("list name"));
      const color = fields.Color;
      if (
        color &&
        (color.type !== "STRING" ||
          (color.value !== null && typeof color.value !== "string"))
      )
        return yield* Effect.fail(fail("list color"));
      return {
        id,
        title: fields.Name.value,
        color: typeof color?.value === "string" ? color.value : null,
        recordChangeTag: record.recordChangeTag ?? null,
      };
    });
  }
  updateList(
    id: string,
    changeTag: string,
    title: string,
  ): Effect.Effect<ReminderListDetails, RemindersError> {
    return Effect.gen({ self: this }, function* () {
      if (!title.trim() || title.length > 4096)
        return yield* Effect.fail(fail("list title", "invalid"));
      const current = yield* this.require(id, "List", changeTag);
      const now = yield* Clock.currentTimeMillis;
      const tokens = yield* Effect.try({
        try: () => tokenMap(current, ["name"], now),
        catch: () => fail("resolution tokens"),
      });
      const tags = yield* this.modify([
        {
          operationType: "update",
          record: {
            recordName: id,
            recordType: "List",
            recordChangeTag: changeTag,
            fields: { Name: str(title), ResolutionTokenMap: str(tokens) },
          },
        },
      ]);
      const result = yield* this.getList(id);
      if (!result || result.title !== title || result.recordChangeTag !== tags.get(id))
        return yield* Effect.fail(fail("list verification"));
      return result;
    });
  }
  private context(reminderId: string, changeTag?: string) {
    return Effect.gen({ self: this }, function* () {
      const reminder = yield* this.require(reminderId, "Reminder", changeTag);
      const listId = refName(reminder.fields?.List);
      if (!listId) return yield* Effect.fail(fail("reminder list"));
      const ids = yield* Effect.try({
        try: () => linkedIds(reminder),
        catch: () => fail("recurrence IDs", "unsupported"),
      });
      const query = yield* this.queryList(listId);
      const records = yield* Schema.decodeUnknownEffect(Schema.Array(RecordSchema))(
        query,
      ).pipe(Effect.mapError(() => fail("recurrence query")));
      const related = new Map<string, RecordValue>();
      for (const record of records) {
        if (record.serverErrorCode || record.errorCode)
          return yield* Effect.fail(fail("recurrence query"));
        if (
          !gone(record) &&
          /recurr|repeat/i.test(record.recordType ?? "") &&
          !refName(record.fields?.Reminder)
        )
          return yield* Effect.fail(fail("recurrence owner"));
        if (
          !gone(record) &&
          /recurr|repeat/i.test(record.recordType ?? "") &&
          refName(record.fields?.Reminder) === reminderId
        )
          related.set(record.recordName, record);
      }
      for (const id of ids) {
        const name = `RecurrenceRule/${id}`;
        const record = yield* this.require(name, "RecurrenceRule");
        if (refName(record.fields?.Reminder) !== reminderId)
          return yield* Effect.fail(fail("recurrence owner"));
        related.set(name, record);
      }
      if (related.size > 100) return yield* Effect.fail(fail("recurrence limit"));
      return { reminder, ids, records: [...related.values()] };
    });
  }
  getRecurrences(
    reminderId: string,
  ): Effect.Effect<ReminderRecurrences, RemindersError> {
    return Effect.gen({ self: this }, function* () {
      const context = yield* this.context(reminderId);
      const rules = yield* Effect.try({
        try: () =>
          context.records.map((record) =>
            record.recordType === "RecurrenceRule"
              ? ruleDetails(record, reminderId)
              : {
                  id: record.recordName,
                  reminderId,
                  recordChangeTag: record.recordChangeTag ?? null,
                  writable: false,
                  recurrence: {
                    supported: false as const,
                    reason: "unknown_fields" as const,
                    fields: ["recordType"],
                  },
                },
          ),
        catch: () => fail("recurrence details"),
      });
      const linked =
        context.ids.length === 1 &&
        context.records.length === 1 &&
        context.records[0].recordName === `RecurrenceRule/${context.ids[0]}` &&
        !Object.keys(context.reminder.fields ?? {}).some(
          (key) => /recurr|repeat/i.test(key) && key !== "RecurrenceRuleIDs",
        );
      return {
        reminderId,
        reminderChangeTag: context.reminder.recordChangeTag!,
        rules: rules.map((rule) => ({ ...rule, writable: rule.writable && linked })),
      };
    });
  }
  private linkFields(reminder: RecordValue, ids: readonly string[], now: number) {
    return Effect.try({
      try: () => ({
        RecurrenceRuleIDs: { type: "STRING_LIST", value: ids },
        LastModifiedDate: { type: "TIMESTAMP", value: now },
        ResolutionTokenMap: str(
          tokenMap(reminder, ["recurrenceRuleIDs", "lastModifiedDate"], now),
        ),
      }),
      catch: () => fail("recurrence link"),
    });
  }
  createRecurrence(
    reminderId: string,
    reminderTag: string,
    ruleId: string,
    input: RecurrenceInput,
  ): Effect.Effect<ReminderRecurrences, RemindersError> {
    return Effect.gen({ self: this }, function* () {
      if (!/^RecurrenceRule\/[A-Za-z0-9-]{8,128}$/.test(ruleId))
        return yield* Effect.fail(fail("rule ID", "invalid"));
      const fields = yield* Effect.try({
        try: () => recurrenceFields(input),
        catch: (cause) =>
          cause instanceof RemindersError ? cause : fail("rule", "invalid"),
      });
      const context = yield* this.context(reminderId, reminderTag);
      // Multiple/unknown recurrence relationships require their own proven semantics.
      if (
        context.ids.length ||
        context.records.length ||
        Object.keys(context.reminder.fields ?? {}).some(
          (key) => /recurr|repeat/i.test(key) && key !== "RecurrenceRuleIDs",
        )
      )
        return yield* Effect.fail(fail("existing recurrence", "unsupported"));
      if (yield* this.lookup(ruleId))
        return yield* Effect.fail(fail("rule ID", "conflict"));
      const links = yield* this.linkFields(
        context.reminder,
        [ruleId.slice("RecurrenceRule/".length)],
        yield* Clock.currentTimeMillis,
      );
      const tags = yield* this.modify([
        {
          operationType: "update",
          record: {
            recordName: reminderId,
            recordType: "Reminder",
            recordChangeTag: reminderTag,
            fields: links,
          },
        },
        {
          operationType: "create",
          record: {
            recordName: ruleId,
            recordType: "RecurrenceRule",
            parent: { recordName: reminderId },
            fields: {
              ...fields,
              Reminder: reference(reminderId),
              Deleted: int(0),
              Imported: int(0),
            },
          },
        },
      ]);
      return yield* this.verifyRecurrence(reminderId, ruleId, tags, fields, false);
    });
  }
  private verifyRecurrence(
    reminderId: string,
    ruleId: string,
    tags: Map<string, string>,
    fields: WriteFields,
    removed: boolean,
  ) {
    return Effect.gen({ self: this }, function* () {
      const reminder = yield* this.require(
        reminderId,
        "Reminder",
        tags.get(reminderId),
      );
      const ids = yield* Effect.try({
        try: () => linkedIds(reminder),
        catch: () => fail("recurrence verification"),
      });
      const rawId = ruleId.slice("RecurrenceRule/".length);
      if (ids.includes(rawId) === removed)
        return yield* Effect.fail(fail("recurrence verification"));
      const rule = yield* this.lookup(ruleId);
      if (removed) {
        if (rule) return yield* Effect.fail(fail("recurrence deletion verification"));
      } else {
        if (
          !rule ||
          rule.recordChangeTag !== tags.get(ruleId) ||
          refName(rule.fields?.Reminder) !== reminderId ||
          Object.entries(fields).some(
            ([name, expected]) =>
              (expected.value === null
                ? rule.fields?.[name]?.value != null
                : JSON.stringify(rule.fields?.[name]?.value) !==
                  JSON.stringify(expected.value)) ||
              (expected.type !== undefined &&
                rule.fields?.[name] !== undefined &&
                rule.fields[name].type !== expected.type),
          )
        )
          return yield* Effect.fail(fail("recurrence verification"));
      }
      const result = yield* this.getRecurrences(reminderId);
      if (
        result.reminderChangeTag !== tags.get(reminderId) ||
        (removed
          ? result.rules.length !== 0
          : result.rules.length !== 1 ||
            result.rules[0].id !== ruleId ||
            result.rules[0].recordChangeTag !== tags.get(ruleId))
      )
        return yield* Effect.fail(fail("recurrence verification"));
      return result;
    });
  }
  updateRecurrence(
    reminderId: string,
    reminderTag: string,
    ruleId: string,
    ruleTag: string,
    input: Partial<RecurrenceInput>,
  ): Effect.Effect<ReminderRecurrences, RemindersError> {
    return Effect.gen({ self: this }, function* () {
      const context = yield* this.context(reminderId, reminderTag);
      const current = yield* this.require(ruleId, "RecurrenceRule", ruleTag);
      const details = yield* Effect.try({
        try: () => ruleDetails(current, reminderId),
        catch: () => fail("recurrence owner"),
      });
      if (
        Object.keys(context.reminder.fields ?? {}).some(
          (key) => /recurr|repeat/i.test(key) && key !== "RecurrenceRuleIDs",
        )
      )
        return yield* Effect.fail(fail("recurrence parent", "unsupported"));
      if (
        !details.writable ||
        !details.recurrence.supported ||
        context.records.length !== 1 ||
        context.records[0].recordName !== ruleId ||
        !context.ids.includes(ruleId.slice("RecurrenceRule/".length))
      )
        return yield* Effect.fail(fail("recurrence update", "unsupported"));
      const merged = { ...details.recurrence.rule, ...input };
      const fields = yield* Effect.try({
        try: () => recurrenceFields(merged, current.fields),
        catch: (cause) =>
          cause instanceof RemindersError
            ? cause
            : fail("recurrence update", "invalid"),
      });
      const keys = Object.keys(input);
      if (!keys.length) return yield* Effect.fail(fail("recurrence update", "invalid"));
      const links = yield* this.linkFields(
        context.reminder,
        context.ids,
        yield* Clock.currentTimeMillis,
      );
      const tags = yield* this.modify([
        {
          operationType: "update",
          record: {
            recordName: reminderId,
            recordType: "Reminder",
            recordChangeTag: reminderTag,
            fields: links,
          },
        },
        {
          operationType: "update",
          record: {
            recordName: ruleId,
            recordType: "RecurrenceRule",
            recordChangeTag: ruleTag,
            fields,
          },
        },
      ]);
      return yield* this.verifyRecurrence(reminderId, ruleId, tags, fields, false);
    });
  }
  removeRecurrence(
    reminderId: string,
    reminderTag: string,
    ruleId: string,
    ruleTag: string,
  ): Effect.Effect<ReminderRecurrences, RemindersError> {
    return Effect.gen({ self: this }, function* () {
      const context = yield* this.context(reminderId, reminderTag);
      const current = yield* this.require(ruleId, "RecurrenceRule", ruleTag);
      const details = yield* Effect.try({
        try: () => ruleDetails(current, reminderId),
        catch: () => fail("recurrence owner"),
      });
      if (
        Object.keys(context.reminder.fields ?? {}).some(
          (key) => /recurr|repeat/i.test(key) && key !== "RecurrenceRuleIDs",
        )
      )
        return yield* Effect.fail(fail("recurrence parent", "unsupported"));
      const rawId = ruleId.slice("RecurrenceRule/".length);
      if (
        !details.writable ||
        context.records.length !== 1 ||
        context.records[0].recordName !== ruleId ||
        !context.ids.includes(rawId)
      )
        return yield* Effect.fail(fail("recurrence removal", "unsupported"));
      const links = yield* this.linkFields(
        context.reminder,
        context.ids.filter((id) => id !== rawId),
        yield* Clock.currentTimeMillis,
      );
      const tags = yield* this.modify([
        {
          operationType: "update",
          record: {
            recordName: reminderId,
            recordType: "Reminder",
            recordChangeTag: reminderTag,
            fields: links,
          },
        },
        {
          operationType: "update",
          record: {
            recordName: ruleId,
            recordType: "RecurrenceRule",
            recordChangeTag: ruleTag,
            fields: { Deleted: int(1), Reminder: reference(reminderId) },
          },
        },
      ]);
      return yield* this.verifyRecurrence(reminderId, ruleId, tags, {}, true);
    });
  }
}
