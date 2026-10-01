import { Clock, Effect, Schema } from "effect";
import { RemindersError, type CkPost, type Reminder } from "./cloudkit.js";
import type { ReminderRecurrences } from "./cloudkitExtras.js";

// Apple's public Reminders web build 2636Build17 uses this mutating query instead
// of a generic Completed update. Independently implemented wire request:
// https://www.icloud.com/applications/reminders2/2636Build17/en-us/main.js
// The query has no change-tag CAS parameter. A caller must reserve durably and
// check current identities/tags before calling; a response alone is NOT proof
// that completion or next-occurrence creation was verified. Never replay this
// request after a lost response or follow a pagination cursor by issuing it again.
const Identity = Schema.String.check(Schema.isMinLength(1), Schema.isMaxLength(256));
const InputSchema = Schema.Struct({
  reminderId: Identity,
  ruleId: Identity,
  timeZone: Schema.String.check(Schema.isMinLength(1), Schema.isMaxLength(128)),
  ownerRecordName: Schema.optional(Identity),
});
export type RecurringCompletionInput = Schema.Schema.Type<typeof InputSchema>;
const CompletionRecord = Schema.Struct({
  recordName: Identity,
  recordType: Schema.optional(Schema.String),
  recordChangeTag: Schema.optional(Identity),
  fields: Schema.optional(
    Schema.Record(
      Schema.String,
      Schema.Struct({ type: Schema.String, value: Schema.Unknown }),
    ),
  ),
  deleted: Schema.optional(Schema.Boolean),
  serverErrorCode: Schema.optional(Schema.String),
  errorCode: Schema.optional(Schema.Number),
});
const Response = Schema.Struct({
  records: Schema.Array(CompletionRecord).check(Schema.isMaxLength(100)),
  continuationMarker: Schema.optional(Schema.NullOr(Schema.String)),
});
export type RecurringCompletionRecord = Schema.Schema.Type<typeof CompletionRecord>;
export interface RecurringCompletionResponse {
  /** Returned records must be freshly looked up and checked against preflight. */
  readonly records: readonly RecurringCompletionRecord[];
  readonly verified: false;
}
const failure = (code: "invalid" | "protocol") =>
  new RemindersError({ operation: "recurring completion query", code });

/** One mutating request. The owning service must reserve before calling this. */
export function requestRecurringCompletion(
  post: CkPost,
  input: RecurringCompletionInput,
): Effect.Effect<RecurringCompletionResponse, RemindersError> {
  return Effect.gen(function* () {
    const request = yield* Schema.decodeUnknownEffect(InputSchema, {
      onExcessProperty: "error",
    })(input).pipe(Effect.mapError(() => failure("invalid")));
    if (
      !request.reminderId.startsWith("Reminder/") ||
      !request.ruleId.startsWith("RecurrenceRule/")
    )
      return yield* Effect.fail(failure("invalid"));
    yield* Effect.try({
      try: () =>
        new Intl.DateTimeFormat("en", { timeZone: request.timeZone }).resolvedOptions(),
      catch: () => failure("invalid"),
    });
    const reference = (recordName: string) => ({
      type: "REFERENCE",
      value: { recordName, action: "VALIDATE" },
    });
    const response = yield* post("/records/query", {
      zoneID: {
        zoneName: "Reminders",
        zoneType: "REGULAR_CUSTOM_ZONE",
        ...(request.ownerRecordName
          ? { ownerRecordName: request.ownerRecordName }
          : {}),
      },
      query: {
        recordType: "CompleteRecurringReminder",
        filterBy: [
          {
            comparator: "EQUALS",
            fieldName: "Reminder",
            fieldValue: reference(request.reminderId),
          },
          {
            comparator: "EQUALS",
            fieldName: "RecurrenceRule",
            fieldValue: reference(request.ruleId),
          },
          {
            comparator: "EQUALS",
            fieldName: "TimeZone",
            fieldValue: { type: "STRING", value: request.timeZone },
          },
        ],
      },
    });
    const decoded = yield* Schema.decodeUnknownEffect(Response)(response).pipe(
      Effect.mapError(() => failure("protocol")),
    );
    if (
      decoded.continuationMarker ||
      decoded.records.length === 0 ||
      new Set(decoded.records.map((record) => record.recordName)).size !==
        decoded.records.length ||
      decoded.records.some((record) => record.serverErrorCode || record.errorCode)
    )
      return yield* Effect.fail(failure("protocol"));
    // Retain bounded records for exact follow-up lookup. Do not infer a completed
    // series, successful mutation, or a next occurrence from their mere presence.
    return { records: decoded.records, verified: false };
  });
}

export type RecurringCompletionTarget = {
  readonly id: string;
  readonly changeTag: string;
  readonly ruleId: string;
  readonly ruleChangeTag: string;
  readonly timeZone: string;
};
export type VerifiedRecurringCompletion = {
  readonly state: "advanced" | "ended";
  readonly verified: true;
  readonly reminderId: string;
  readonly reminderChangeTag: string;
  readonly completedReminderId: string;
  readonly completedReminderChangeTag: string;
  readonly ruleId: string;
  readonly timeZone: string;
  readonly previousDueDate: number;
  readonly nextDueDate: number | null;
};
type CompletionDependencies = {
  readonly post: CkPost;
  readonly getReminder: (id: string) => Effect.Effect<Reminder | null, RemindersError>;
  readonly getRecurrences: (
    id: string,
  ) => Effect.Effect<ReminderRecurrences, RemindersError>;
};
const unchangedContent = (before: Reminder, after: Reminder) =>
  ["title", "description", "listId", "priority", "flagged", "allDay"].every(
    (field) => before[field as keyof Reminder] === after[field as keyof Reminder],
  );
const outcomeUncertain = () =>
  new RemindersError({ operation: "recurring completion outcome", code: "uncertain" });

function previewMatches(
  record: RecurringCompletionRecord,
  reminder: Reminder,
): boolean {
  const fields = record.fields;
  if (
    fields?.Completed?.type !== "INT64" ||
    fields.Completed.value !== (reminder.completed ? 1 : 0) ||
    fields.DueDate?.type !== "TIMESTAMP" ||
    fields.DueDate.value !== reminder.dueDate
  )
    return false;
  const completion = fields.CompletionDate;
  // Apple's advanced-root preview omits a null CompletionDate. Completed
  // occurrences must provide their explicit timestamp; missing is not success.
  return !completion
    ? !reminder.completed && reminder.completedDate === null
    : completion.type === "TIMESTAMP" && completion.value === reminder.completedDate;
}

/** Call only inside the durable mutation reservation and account serialization. */
export function completeRecurringOccurrence(
  deps: CompletionDependencies,
  target: RecurringCompletionTarget,
): Effect.Effect<VerifiedRecurringCompletion, RemindersError> {
  return Effect.gen(function* () {
    const before = yield* deps.getReminder(target.id);
    if (!before)
      return yield* Effect.fail(
        new RemindersError({ operation: "recurring completion", code: "not_found" }),
      );
    if (before.recordChangeTag !== target.changeTag)
      return yield* Effect.fail(
        new RemindersError({ operation: "recurring completion", code: "conflict" }),
      );
    if (
      before.completed ||
      !before.recurring ||
      before.deleted ||
      before.dueDate === null
    )
      return yield* Effect.fail(
        new RemindersError({ operation: "recurring completion", code: "unsupported" }),
      );
    const originalDueDate = before.dueDate;
    const related = yield* deps.getRecurrences(target.id);
    if (
      related.reminderId !== target.id ||
      related.reminderChangeTag !== target.changeTag ||
      related.rules.length !== 1 ||
      related.rules[0].id !== target.ruleId ||
      related.rules[0].recordChangeTag !== target.ruleChangeTag
    )
      return yield* Effect.fail(
        new RemindersError({ operation: "recurring completion", code: "conflict" }),
      );
    const originalRule = related.rules[0];
    if (!originalRule.writable || !originalRule.recurrence.supported)
      return yield* Effect.fail(
        new RemindersError({ operation: "recurring completion", code: "unsupported" }),
      );
    // Validate timezone before issuing the mutation. There is no protocol CAS;
    // concurrent native-client edits are detected only by subsequent verification.
    yield* Effect.try({
      try: () => new Intl.DateTimeFormat("en", { timeZone: target.timeZone }),
      catch: () => failure("invalid"),
    });
    const started = yield* Clock.currentTimeMillis;
    return yield* Effect.gen(function* () {
      const response = yield* requestRecurringCompletion(deps.post, {
        reminderId: target.id,
        ruleId: target.ruleId,
        timeZone: target.timeZone,
      });
      const returnedReminders = response.records.filter(
        (record) => record.recordType === "Reminder",
      );
      const root = returnedReminders.find((record) => record.recordName === target.id);
      const clone = returnedReminders.find((record) => record.recordName !== target.id);
      // Query records are previews: live responses inherited the old root tag on
      // both root and clone. Bind their exact IDs and typed completion/date values
      // to fresh lookups, never their non-authoritative recordChangeTag fields.
      if (returnedReminders.length === 1 && root && !root.deleted) {
        const current = yield* deps.getReminder(target.id);
        const finished = yield* Clock.currentTimeMillis;
        const rule = originalRule.recurrence;
        if (!rule.supported) return yield* Effect.fail(outcomeUncertain());
        const endDate = rule.rule.endDate;
        const civilDate = (date: number) =>
          new Intl.DateTimeFormat("en-CA", {
            timeZone: target.timeZone,
            year: "numeric",
            month: "2-digit",
            day: "2-digit",
          }).format(date);
        // Proven finite-series shape: same root completes without a clone. Limit
        // inference to simple rules with at most one occurrence per civil date,
        // whose inclusive end boundary lies on this occurrence's civil date.
        // Other end/count forms remain uncertain rather than guessing a calendar.
        const simple =
          ["daily", "weekly", "monthly", "yearly"].includes(rule.rule.frequency) &&
          ![
            rule.rule.daysOfWeek,
            rule.rule.daysOfMonth,
            rule.rule.daysOfYear,
            rule.rule.weeksOfYear,
            rule.rule.monthsOfYear,
            rule.rule.setPositions,
          ].some((values) => values?.length);
        if (
          !simple ||
          endDate === null ||
          endDate === undefined ||
          endDate < originalDueDate ||
          civilDate(endDate) !== civilDate(originalDueDate) ||
          !current ||
          current.id !== target.id ||
          current.deleted ||
          !current.completed ||
          !current.recurring ||
          current.dueDate !== originalDueDate ||
          current.startDate !== before.startDate ||
          current.recordChangeTag === target.changeTag ||
          !previewMatches(root, current) ||
          !unchangedContent(before, current) ||
          current.completedDate === null ||
          current.completedDate < started - 120_000 ||
          current.completedDate > finished + 120_000
        )
          return yield* Effect.fail(outcomeUncertain());
        const afterRule = yield* deps.getRecurrences(target.id);
        if (
          afterRule.reminderChangeTag !== current.recordChangeTag ||
          afterRule.rules.length !== 1 ||
          afterRule.rules[0].id !== originalRule.id ||
          afterRule.rules[0].recordChangeTag !== originalRule.recordChangeTag ||
          !afterRule.rules[0].writable ||
          JSON.stringify(afterRule.rules[0].recurrence) !==
            JSON.stringify(originalRule.recurrence)
        )
          return yield* Effect.fail(outcomeUncertain());
        return {
          state: "ended",
          verified: true,
          reminderId: current.id,
          reminderChangeTag: current.recordChangeTag,
          completedReminderId: current.id,
          completedReminderChangeTag: current.recordChangeTag,
          ruleId: target.ruleId,
          timeZone: target.timeZone,
          previousDueDate: originalDueDate,
          nextDueDate: null,
        } as const;
      }
      if (
        returnedReminders.length !== 2 ||
        !root ||
        !clone ||
        root.deleted ||
        clone.deleted
      )
        return yield* Effect.fail(outcomeUncertain());
      const current = yield* deps.getReminder(target.id);
      const completed = yield* deps.getReminder(clone.recordName);
      const finished = yield* Clock.currentTimeMillis;
      if (
        !current ||
        !completed ||
        current.id !== target.id ||
        completed.id !== clone.recordName ||
        current.recordChangeTag === target.changeTag ||
        !previewMatches(root, current) ||
        !previewMatches(clone, completed) ||
        current.deleted ||
        completed.deleted ||
        current.completed ||
        !current.recurring ||
        current.dueDate === null ||
        current.dueDate <= originalDueDate ||
        !completed.completed ||
        completed.recurring ||
        completed.dueDate !== originalDueDate ||
        completed.startDate !== before.startDate ||
        completed.completedDate === null ||
        completed.completedDate < started - 120_000 ||
        completed.completedDate > finished + 120_000 ||
        !unchangedContent(before, current) ||
        !unchangedContent(before, completed) ||
        (before.startDate === null
          ? current.startDate !== null
          : current.startDate !== before.startDate + current.dueDate - originalDueDate)
      )
        return yield* Effect.fail(outcomeUncertain());
      const afterRule = yield* deps.getRecurrences(target.id);
      if (
        afterRule.reminderChangeTag !== current.recordChangeTag ||
        afterRule.rules.length !== 1 ||
        afterRule.rules[0].id !== originalRule.id ||
        afterRule.rules[0].recordChangeTag !== originalRule.recordChangeTag ||
        !afterRule.rules[0].writable ||
        !afterRule.rules[0].recurrence.supported ||
        JSON.stringify(afterRule.rules[0].recurrence) !==
          JSON.stringify(originalRule.recurrence)
      )
        return yield* Effect.fail(outcomeUncertain());
      const stableCompleted = yield* deps.getReminder(completed.id);
      if (
        !stableCompleted ||
        stableCompleted.recordChangeTag !== completed.recordChangeTag ||
        !unchangedContent(completed, stableCompleted) ||
        stableCompleted.deleted ||
        stableCompleted.recurring ||
        !stableCompleted.completed ||
        stableCompleted.dueDate !== completed.dueDate ||
        stableCompleted.startDate !== completed.startDate ||
        stableCompleted.completedDate !== completed.completedDate
      )
        return yield* Effect.fail(outcomeUncertain());
      return {
        state: "advanced",
        verified: true,
        reminderId: current.id,
        reminderChangeTag: current.recordChangeTag,
        completedReminderId: completed.id,
        completedReminderChangeTag: completed.recordChangeTag,
        ruleId: target.ruleId,
        timeZone: target.timeZone,
        previousDueDate: originalDueDate,
        nextDueDate: current.dueDate,
      } as const;
    }).pipe(Effect.mapError(() => outcomeUncertain()));
  });
}
