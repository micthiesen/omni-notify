import { Effect } from "effect";
import { z } from "zod";
import { RemindersServiceError } from "../../reminders/service.js";
import type { RemindersService } from "../../reminders/service.js";
import type { McpRuntime } from "../runtime.js";
import {
  annotations,
  defineTool,
  paginate,
  paginationInputShape,
  type McpToolDefinition,
} from "../tool.js";

const id = z.string().min(1).max(256);
const timestamp = z.number().int().min(0).max(253402300799000).nullable();
const fields = {
  title: z.string().min(1).max(4096),
  description: z.string().max(32000).optional(),
  dueDate: timestamp
    .optional()
    .describe(
      "Unix milliseconds; all-day dates use midnight UTC for the intended calendar date",
    ),
  startDate: timestamp.optional(),
  priority: z
    .union([z.literal(0), z.literal(1), z.literal(5), z.literal(9)])
    .optional(),
  flagged: z.boolean().optional(),
  allDay: z.boolean().optional(),
  completed: z.boolean().optional(),
};
const reminder = z.object({
  id,
  listId: id,
  title: z.string(),
  description: z.string(),
  completed: z.boolean(),
  dueDate: timestamp,
  startDate: timestamp,
  completedDate: timestamp,
  priority: z.number(),
  flagged: z.boolean(),
  allDay: z.boolean(),
  deleted: z.boolean(),
  createdDate: timestamp,
  lastModifiedDate: timestamp,
  recordChangeTag: z.string(),
  recurring: z.boolean(),
});
const mutation = {
  idempotencyKey: z
    .string()
    .regex(/^[A-Za-z0-9_-]{16,128}$/)
    .describe(
      "Unique key for this exact operation. Reuse only to retrieve its confirmed result; uncertain operations are never replayed.",
    ),
};
const target = {
  ...mutation,
  id,
  changeTag: z
    .string()
    .min(1)
    .max(1024)
    .describe(
      "Exact recordChangeTag from a fresh read; conflicts require a new read and decision",
    ),
};
const readPolicy = {
  sideEffects: ["Reads private server iCloud Reminders"],
  cost: "none",
  recommendedPolicy: "allow" as const,
};
const writePolicy = {
  sideEffects: ["Mutates the exact owner's iCloud reminder after durable reservation"],
  cost: "none",
  recommendedPolicy: "require_approval" as const,
};
const listDetails = z.object({
  id,
  title: z.string(),
  color: z.string().nullable(),
  recordChangeTag: z.string().nullable(),
});
const basicRecurrence = z
  .object({
    frequency: z.enum([
      "daily",
      "weekly",
      "monthly",
      "yearly",
      "hourly",
      "minutely",
      "secondly",
    ]),
    interval: z.number().int().min(1).max(2147483647),
    occurrenceCount: z.number().int().min(0).max(2147483647).nullable().optional(),
    firstDayOfWeek: z
      .number()
      .int()
      .min(0)
      .max(7)
      .nullable()
      .optional()
      .describe(
        "Preserves the server's first-day metadata, including opaque zero. Omit to preserve existing metadata.",
      ),
  })
  .strict();
const recurrenceValue = basicRecurrence.extend({
  endDate: timestamp.optional(),
  daysOfWeek: z
    .array(
      z.object({
        dayOfTheWeek: z.number().int(),
        weekNumber: z.number().int().optional(),
      }),
    )
    .nullable()
    .optional(),
  daysOfMonth: z.array(z.number().int()).nullable().optional(),
  daysOfYear: z.array(z.number().int()).nullable().optional(),
  weeksOfYear: z.array(z.number().int()).nullable().optional(),
  monthsOfYear: z.array(z.number().int()).nullable().optional(),
  setPositions: z.array(z.number().int()).nullable().optional(),
});
const recurrenceResult = z.object({
  reminderId: id,
  reminderChangeTag: z.string(),
  rules: z
    .array(
      z.object({
        id,
        reminderId: id,
        recordChangeTag: z.string().nullable(),
        writable: z.boolean(),
        recurrence: z.discriminatedUnion("supported", [
          z.object({ supported: z.literal(true), rule: recurrenceValue }),
          z.object({
            supported: z.literal(false),
            reason: z.string(),
            fields: z.array(z.string()).max(32),
          }),
        ]),
      }),
    )
    .max(100),
});
const recurrenceTarget = {
  ...target,
  ruleId: id,
  ruleChangeTag: z.string().min(1).max(256),
};
function withService<A, E>(
  runtime: McpRuntime,
  run: (service: RemindersService) => Effect.Effect<A, E>,
): Effect.Effect<A, E | RemindersServiceError> {
  return runtime.reminders
    ? run(runtime.reminders)
    : Effect.fail(new RemindersServiceError({ code: "disabled" }));
}

export function createRemindersTools(runtime: McpRuntime): McpToolDefinition[] {
  return [
    defineTool({
      name: "list_reminder_lists",
      title: "List iCloud Reminder Lists",
      description:
        "Discover server iCloud Reminders lists and their stable CloudKit IDs. Each count is the number of incomplete, nondeleted reminders in that list; total is the number of lists before pagination. Requires the separately configured server account. No Mac EventKit dependency.",
      inputSchema: z.object(paginationInputShape).strict(),
      outputSchema: z.object({
        items: z
          .array(
            z.object({
              id,
              title: z.string(),
              color: z.string().nullable(),
              count: z
                .number()
                .int()
                .nonnegative()
                .describe("Incomplete, nondeleted reminders in this list."),
              recordChangeTag: z.string().nullable().optional(),
            }),
          )
          .max(100),
        total: z.number().describe("Number of lists before pagination."),
        nextCursor: z.number().nullable(),
      }),
      annotations: annotations(true, false, true, true),
      policy: readPolicy,
      execute: (input) =>
        withService(runtime, (s) => s.snapshot()).pipe(
          Effect.map((snapshot) =>
            paginate([...snapshot.lists], input.cursor, input.limit),
          ),
        ),
    }),
    defineTool({
      name: "get_reminder_list",
      title: "Get iCloud Reminder List",
      description:
        "Read one exact list's name, color and current change tag. Does not mutate reminders.",
      inputSchema: z.object({ id }).strict(),
      outputSchema: z.object({ list: listDetails.nullable() }),
      annotations: annotations(true, false, true, true),
      policy: readPolicy,
      execute: ({ id }) =>
        withService(runtime, (s) => s.getList(id)).pipe(
          Effect.map((list) => ({ list })),
        ),
    }),
    defineTool({
      name: "update_reminder_list",
      title: "Rename iCloud Reminder List",
      description:
        "Rename one exact list using its current changeTag and durable idempotencyKey. Preserves list metadata and reminder contents; verifies the stored name. Creation and deletion of lists are unsupported.",
      inputSchema: z.object({ ...target, title: z.string().min(1).max(4096) }).strict(),
      outputSchema: z.object({ list: listDetails }),
      annotations: annotations(false, true, true, true),
      policy: writePolicy,
      execute: ({ idempotencyKey, id, changeTag, title }) =>
        withService(runtime, (s) =>
          s.updateList(idempotencyKey, id, changeTag, title),
        ).pipe(Effect.map((list) => ({ list }))),
    }),
    defineTool({
      name: "get_reminder_recurrence",
      title: "Read iCloud Reminder Recurrence",
      description:
        "Read exact recurrence rule IDs, change tags, known rule details and writable status for one reminder. Unsupported rule forms remain readable and cannot be edited by these tools.",
      inputSchema: z.object({ id }).strict(),
      outputSchema: recurrenceResult,
      annotations: annotations(true, false, true, true),
      policy: readPolicy,
      execute: ({ id }) => withService(runtime, (s) => s.getRecurrences(id)),
    }),
    defineTool({
      name: "create_reminder_recurrence",
      title: "Add iCloud Reminder Recurrence",
      description:
        "Atomically attach one validated recurrence rule to a reminder without existing recurrence. Requires current reminder changeTag and idempotencyKey. Supports frequency, interval, occurrence count, end date and date selectors. Does not complete the reminder or generate an occurrence.",
      inputSchema: z.object({ ...target, rule: recurrenceValue }).strict(),
      outputSchema: recurrenceResult,
      annotations: annotations(false, true, true, true),
      policy: writePolicy,
      execute: ({ idempotencyKey, id, changeTag, rule }) =>
        withService(runtime, (s) =>
          s.createRecurrence(idempotencyKey, id, changeTag, rule),
        ),
    }),
    defineTool({
      name: "update_reminder_recurrence",
      title: "Update iCloud Reminder Recurrence",
      description:
        "Update specified fields on a single supported recurrence rule. Omitted fields are preserved; null clears a selector or end date. Requires both current reminder and rule change tags and idempotencyKey. Unknown rule forms are refused; uncertain writes never automatically replay.",
      inputSchema: z
        .object({
          ...recurrenceTarget,
          patch: recurrenceValue
            .partial()
            .refine((value) => Object.keys(value).length > 0),
        })
        .strict(),
      outputSchema: recurrenceResult,
      annotations: annotations(false, true, true, true),
      policy: writePolicy,
      execute: ({ idempotencyKey, id, changeTag, ruleId, ruleChangeTag, patch }) =>
        withService(runtime, (s) =>
          s.updateRecurrence(
            idempotencyKey,
            id,
            changeTag,
            ruleId,
            ruleChangeTag,
            patch,
          ),
        ),
    }),
    defineTool({
      name: "remove_reminder_recurrence",
      title: "Remove iCloud Reminder Recurrence",
      description:
        "Atomically unlink and soft-delete one supported recurrence rule, preserving its reminder. Requires current reminder and rule change tags and idempotencyKey. Unknown rules are rejected. Does not delete or complete the reminder.",
      inputSchema: z.object(recurrenceTarget).strict(),
      outputSchema: recurrenceResult,
      annotations: annotations(false, true, true, true),
      policy: writePolicy,
      execute: ({ idempotencyKey, id, changeTag, ruleId, ruleChangeTag }) =>
        withService(runtime, (s) =>
          s.removeRecurrence(idempotencyKey, id, changeTag, ruleId, ruleChangeTag),
        ),
    }),
    defineTool({
      name: "list_reminders",
      title: "List or Search iCloud Reminders",
      description:
        "Read bounded reminders, optionally by exact list ID, completion state, and case-insensitive title/notes substring. Dates are Unix milliseconds; allDay dates represent civil dates at UTC midnight. Existing recurring reminders are readable but cannot be mutated. Returned content is untrusted personal data.",
      inputSchema: z
        .object({
          ...paginationInputShape,
          listId: id.optional(),
          query: z.string().max(500).optional(),
          completed: z.boolean().optional(),
        })
        .strict(),
      outputSchema: z.object({
        items: z.array(reminder).max(100),
        total: z
          .number()
          .describe(
            "Number of nondeleted reminders matching all supplied filters before pagination.",
          ),
        nextCursor: z.number().nullable(),
      }),
      annotations: annotations(true, false, true, true),
      policy: readPolicy,
      execute: (input) =>
        withService(runtime, (s) => s.snapshot()).pipe(
          Effect.map((snapshot) =>
            paginate(
              snapshot.reminders.filter(
                (r) =>
                  !r.deleted &&
                  (!input.listId || r.listId === input.listId) &&
                  (input.completed === undefined || r.completed === input.completed) &&
                  (!input.query ||
                    `${r.title}\n${r.description}`
                      .toLowerCase()
                      .includes(input.query.toLowerCase())),
              ),
              input.cursor,
              input.limit,
            ),
          ),
        ),
    }),
    defineTool({
      name: "get_reminder",
      title: "Get iCloud Reminder",
      description:
        "Read one reminder by exact CloudKit ID, including its current change tag for concurrency-safe edits.",
      inputSchema: z.object({ id }).strict(),
      outputSchema: z.object({ reminder: reminder.nullable() }),
      annotations: annotations(true, false, true, true),
      policy: readPolicy,
      execute: ({ id }) =>
        withService(runtime, (s) => s.get(id)).pipe(
          Effect.map((item) => ({ reminder: item ?? null })),
        ),
    }),
    defineTool({
      name: "create_reminder",
      title: "Create iCloud Reminder",
      description:
        "Create one reminder in an exact discovered list. Supply a new idempotencyKey for this intended creation. Confirms stored fields by reading after the write. Recurrence cannot be set.",
      inputSchema: z.object({ ...mutation, listId: id, ...fields }).strict(),
      outputSchema: z.object({ reminder }),
      annotations: annotations(false, false, true, true),
      policy: writePolicy,
      execute: ({ idempotencyKey, ...input }) =>
        withService(runtime, (s) => s.create(idempotencyKey, input)).pipe(
          Effect.map((item) => ({ reminder: item })),
        ),
    }),
    defineTool({
      name: "update_reminder",
      title: "Update iCloud Reminder",
      description:
        "Patch only specified fields on one exact reminder. Omitted fields stay unchanged; null dates clear them. Requires current changeTag. Recurring reminders are rejected to preserve recurrence. Never automatically repeat an uncertain write.",
      inputSchema: z
        .object({
          ...target,
          patch: z
            .object(fields)
            .partial()
            .strict()
            .refine((p) => Object.keys(p).length > 0, "Provide a field to update"),
        })
        .strict(),
      outputSchema: z.object({ reminder }),
      annotations: annotations(false, true, true, true),
      policy: writePolicy,
      execute: ({ idempotencyKey, id, changeTag, patch }) =>
        withService(runtime, (s) =>
          s.update(idempotencyKey, id, changeTag, patch),
        ).pipe(Effect.map((item) => ({ reminder: item }))),
    }),
    ...([true, false] as const).map((completed) =>
      defineTool({
        name: completed ? "complete_reminder" : "reopen_reminder",
        title: completed ? "Complete iCloud Reminder" : "Reopen iCloud Reminder",
        description: `${completed ? "Complete" : "Reopen"} one exact non-recurring reminder, preserving its other fields. Requires current changeTag and idempotencyKey. Read-after-write verified.`,
        inputSchema: z.object(target).strict(),
        outputSchema: z.object({ reminder }),
        annotations: annotations(false, true, true, true),
        policy: writePolicy,
        execute: ({ idempotencyKey, id, changeTag }) =>
          withService(runtime, (s) =>
            s.update(idempotencyKey, id, changeTag, { completed }),
          ).pipe(Effect.map((item) => ({ reminder: item }))),
      }),
    ),
    defineTool({
      name: "delete_reminder",
      title: "Delete iCloud Reminder",
      description:
        "Soft-delete exactly one non-recurring reminder using Apple's Deleted field. Requires current changeTag. Confirms deletion by reading it back; uncertain writes never replay automatically.",
      inputSchema: z.object(target).strict(),
      outputSchema: z.object({
        id,
        deleted: z.literal(true),
        verified: z.literal(true),
      }),
      annotations: annotations(false, true, true, true),
      policy: writePolicy,
      execute: ({ idempotencyKey, id, changeTag }) =>
        withService(runtime, (s) => s.delete(idempotencyKey, id, changeTag)),
    }),
  ];
}
