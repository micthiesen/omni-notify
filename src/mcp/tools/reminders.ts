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
        "Discover server iCloud Reminders lists and their stable CloudKit IDs. Requires the separately configured server account. No Mac EventKit dependency.",
      inputSchema: z.object(paginationInputShape).strict(),
      outputSchema: z.object({
        items: z
          .array(
            z.object({
              id,
              title: z.string(),
              color: z.string().nullable(),
              count: z.number(),
            }),
          )
          .max(100),
        total: z.number(),
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
        total: z.number(),
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
