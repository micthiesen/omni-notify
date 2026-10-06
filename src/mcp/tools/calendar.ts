import { createHash } from "node:crypto";
import { Clock, Effect } from "effect";
import { z } from "zod";
import { buildICalendar } from "../../calendar-events/caldav/ics.js";
import {
  createCalendarEventEffect,
  deleteCalendarEventEffect,
  discoverCaldavSessionEffect,
  getCaldavProvider,
  updateCalendarEventEffect,
} from "../../calendar-events/caldav/index.js";
import { isValidTimeZone } from "../../calendar-events/extraction/sanitize.js";
import type { ExtractedCalendarEvent } from "../../calendar-events/extraction/schema.js";
import {
  type CreatedCalendarEventData,
  computeEventHash,
  getTrackedCalendarEventEffect,
  getTrackedCalendarEventsEffect,
  hasCreatedEventEffect,
  hasEventChanged,
  markEventCancelledEffect,
  recordCreatedEventEffect,
  replaceCreatedEventEffect,
} from "../../calendar-events/persistence.js";
import type { McpRuntime } from "../runtime.js";
import {
  annotations,
  defineTool,
  emptyInputSchema,
  type McpToolDefinition,
  paginate,
  paginationInputShape,
} from "../tool.js";

const dateSchema = z
  .string()
  .regex(/^\d{4}-\d{2}-\d{2}$/, "Expected an ISO date (YYYY-MM-DD)")
  .refine((value) => {
    const parsed = new Date(`${value}T00:00:00Z`);
    return (
      !Number.isNaN(parsed.getTime()) && parsed.toISOString().slice(0, 10) === value
    );
  }, "Invalid date");

const timeSchema = z
  .string()
  .regex(/^([01]\d|2[0-3]):[0-5]\d$/, "Expected a 24-hour time (HH:MM)");

const durationSchema = z
  .string()
  .regex(
    /^P(?=\d|T\d)(?:\d+D)?(?:T(?:\d+H)?(?:\d+M)?(?:\d+S)?)?$/,
    "Invalid ISO 8601 duration",
  );

const recurrenceSchema = z
  .object({
    frequency: z.enum(["daily", "weekly", "monthly"]),
    until: dateSchema,
  })
  .strict();

const calendarEventInputSchema = z
  .object({
    title: z.string().trim().min(1).max(200),
    startDate: dateSchema,
    startTime: timeSchema.optional(),
    endDate: dateSchema.optional(),
    endTime: timeSchema.optional(),
    allDay: z.boolean(),
    location: z.string().trim().min(1).max(300).optional(),
    timeZone: z
      .string()
      .max(100)
      .refine(isValidTimeZone, "Expected a valid IANA time zone")
      .optional(),
    description: z.string().max(2_000).optional(),
    duration: durationSchema.optional(),
    reminderMinutes: z.number().int().min(0).max(40_320).optional(),
    recurrence: recurrenceSchema.optional(),
  })
  .strict()
  .superRefine((event, ctx) => {
    if (!event.allDay && !event.startTime) {
      ctx.addIssue({
        code: "custom",
        path: ["startTime"],
        message: "Timed events require startTime",
      });
    }
    if (event.allDay && (event.startTime || event.endTime || event.duration)) {
      ctx.addIssue({
        code: "custom",
        path: ["allDay"],
        message: "All-day events cannot include times or duration",
      });
    }
    if (event.endTime && !event.startTime) {
      ctx.addIssue({
        code: "custom",
        path: ["endTime"],
        message: "endTime requires startTime",
      });
    }
    if (event.endTime && event.duration) {
      ctx.addIssue({
        code: "custom",
        path: ["duration"],
        message: "Use either endTime or duration, not both",
      });
    }
    if (event.endDate && event.endDate < event.startDate) {
      ctx.addIssue({
        code: "custom",
        path: ["endDate"],
        message: "endDate cannot precede startDate",
      });
    }
    if (event.recurrence && event.recurrence.until < event.startDate) {
      ctx.addIssue({
        code: "custom",
        path: ["recurrence", "until"],
        message: "recurrence.until cannot precede startDate",
      });
    }
  });

const calendarEventPatchSchema = z
  .object({
    title: z.string().trim().min(1).max(200).optional(),
    startDate: dateSchema.optional(),
    startTime: timeSchema.nullable().optional(),
    endDate: dateSchema.nullable().optional(),
    endTime: timeSchema.nullable().optional(),
    allDay: z.boolean().optional(),
    location: z.string().trim().min(1).max(300).nullable().optional(),
    timeZone: z
      .string()
      .max(100)
      .refine(isValidTimeZone, "Expected a valid IANA time zone")
      .nullable()
      .optional(),
    description: z.string().max(2_000).nullable().optional(),
    duration: durationSchema.nullable().optional(),
    reminderMinutes: z.number().int().min(0).max(40_320).nullable().optional(),
    recurrence: recurrenceSchema.nullable().optional(),
  })
  .strict()
  .refine((value) => Object.keys(value).length > 0, "At least one change is required");

const trackedEventSchema = z.object({
  eventHash: z.string(),
  calendarEventId: z.string(),
  sourceEmailId: z.string(),
  title: z.string(),
  startDate: z.string(),
  startTime: z.string().nullable(),
  endDate: z.string().nullable(),
  endTime: z.string().nullable(),
  allDay: z.boolean(),
  location: z.string().nullable(),
  timeZone: z.string().nullable(),
  description: z.string().nullable(),
  duration: z.string().nullable(),
  reminderMinutes: z.number().nullable(),
  recurrence: recurrenceSchema.nullable(),
  createdAt: z.number(),
  status: z.enum(["active", "cancelled"]),
});

function serializeTrackedEvent(
  event: CreatedCalendarEventData,
): z.infer<typeof trackedEventSchema> {
  return {
    eventHash: event.eventHash,
    calendarEventId: event.calendarEventId,
    sourceEmailId: event.emailId,
    title: event.title,
    startDate: event.startDate,
    startTime: event.startTime ?? null,
    endDate: event.endDate ?? null,
    endTime: event.endTime ?? null,
    allDay: event.allDay,
    location: event.location ?? null,
    timeZone: event.timeZone ?? null,
    description: event.description ?? null,
    duration: event.duration ?? null,
    reminderMinutes: event.reminderMinutes ?? null,
    recurrence: event.recurrence ?? null,
    createdAt: event.createdAt,
    status: event.status === "cancelled" ? "cancelled" : "active",
  };
}

function toExtractedEvent(
  event: z.infer<typeof calendarEventInputSchema>,
): ExtractedCalendarEvent {
  return { action: "create", ...event };
}

const getTrackedEventOrFailEffect = Effect.fn("McpCalendar.getTrackedEvent")(function* (
  eventHash: string,
) {
  const event = yield* getTrackedCalendarEventEffect(eventHash);
  if (!event) {
    return yield* Effect.fail(
      new Error(`Unknown tracked calendar event: ${eventHash}`),
    );
  }
  return event;
});

export function createCalendarTools(runtime: McpRuntime): McpToolDefinition[] {
  const logger = runtime.logger.extend("MCP:Calendar");

  return [
    defineTool({
      name: "calendar_events_list",
      title: "List Tracked Calendar Events",
      description:
        "List Omni's locally tracked CalDAV events with bounded filtering and pagination. This does not enumerate unrelated events directly from the remote calendar.",
      inputSchema: z
        .object({
          ...paginationInputShape,
          query: z.string().trim().min(1).max(200).optional(),
          from: dateSchema.optional(),
          through: dateSchema.optional(),
          status: z.enum(["active", "cancelled", "all"]).default("active"),
        })
        .strict(),
      outputSchema: z.object({
        items: z.array(trackedEventSchema),
        nextCursor: z.number().nullable(),
        total: z.number(),
      }),
      annotations: annotations(true, false, true, false),
      policy: { sideEffects: [], cost: "None", recommendedPolicy: "allow" },
      execute: (input) =>
        Effect.gen(function* () {
          const query = input.query?.toLowerCase();
          const events = (yield* getTrackedCalendarEventsEffect())
            .filter((event) => {
              const status = event.status === "cancelled" ? "cancelled" : "active";
              if (input.status !== "all" && input.status !== status) return false;
              if (input.from && event.startDate < input.from) return false;
              if (input.through && event.startDate > input.through) return false;
              if (
                query &&
                !`${event.title}\n${event.location ?? ""}\n${event.description ?? ""}`
                  .toLowerCase()
                  .includes(query)
              ) {
                return false;
              }
              return true;
            })
            .sort((a, b) =>
              `${a.startDate}T${a.startTime ?? "00:00"}`.localeCompare(
                `${b.startDate}T${b.startTime ?? "00:00"}`,
              ),
            )
            .map(serializeTrackedEvent);
          return paginate(events, input.cursor, input.limit);
        }),
    }),
    defineTool({
      name: "calendar_event_get",
      title: "Get Tracked Calendar Event",
      description:
        "Get one Omni-tracked calendar event by eventHash, including its stable CalDAV UID and local status.",
      inputSchema: z.object({ eventHash: z.string().min(1).max(1_000) }).strict(),
      outputSchema: z.object({ event: trackedEventSchema }),
      annotations: annotations(true, false, true, false),
      policy: { sideEffects: [], cost: "None", recommendedPolicy: "allow" },
      execute: (input) =>
        Effect.gen(function* () {
          return {
            event: serializeTrackedEvent(
              yield* getTrackedEventOrFailEffect(input.eventHash),
            ),
          };
        }),
    }),
    defineTool({
      name: "calendar_status",
      title: "Inspect Tracked Calendar Status",
      description:
        "Report the configured CalDAV provider and local tracked-event counts without contacting the provider or revealing calendar URLs or credentials.",
      inputSchema: emptyInputSchema,
      outputSchema: z.object({
        configured: z.boolean(),
        provider: z.literal("icloud").nullable(),
        tracked: z.object({
          active: z.number(),
          cancelled: z.number(),
          total: z.number(),
        }),
      }),
      annotations: annotations(true, false, true, false),
      policy: { sideEffects: [], cost: "None", recommendedPolicy: "allow" },
      execute: () =>
        Effect.gen(function* () {
          const events = yield* getTrackedCalendarEventsEffect();
          const cancelled = events.filter(
            (event) => event.status === "cancelled",
          ).length;
          const provider = getCaldavProvider();
          return {
            configured: Boolean(provider),
            provider: provider ?? null,
            tracked: {
              active: events.length - cancelled,
              cancelled,
              total: events.length,
            },
          };
        }),
    }),
    defineTool({
      name: "calendar_event_preview",
      title: "Preview Calendar Event",
      description:
        "Validate a proposed event and render the exact bounded iCalendar payload Omni would write. No local or remote state changes.",
      inputSchema: z.object({ event: calendarEventInputSchema }).strict(),
      outputSchema: z.object({
        eventHash: z.string(),
        duplicateTrackedEvent: z.boolean(),
        iCalendar: z.string(),
      }),
      annotations: annotations(true, false, true, false),
      policy: { sideEffects: [], cost: "None", recommendedPolicy: "allow" },
      execute: (input) =>
        Effect.gen(function* () {
          const event = toExtractedEvent(input.event);
          const generatedAt = yield* Clock.currentTimeMillis;
          const eventHash = computeEventHash(
            event.title,
            event.startDate,
            event.startTime,
          );
          return {
            eventHash,
            duplicateTrackedEvent: yield* hasCreatedEventEffect(eventHash),
            iCalendar: buildICalendar(event, "preview@omni-notify", generatedAt),
          };
        }),
    }),
    defineTool({
      name: "calendar_event_create",
      title: "Create Calendar Event",
      description:
        "Create and locally track an event in the configured CalDAV calendar. Content-hash deduplication makes identical repeated calls no-ops. This external calendar mutation requires approval.",
      inputSchema: z.object({ event: calendarEventInputSchema }).strict(),
      outputSchema: z.object({
        status: z.enum(["created", "already_exists", "reconciled"]),
        event: trackedEventSchema,
      }),
      annotations: annotations(false, false, true, true),
      policy: {
        sideEffects: [
          "Creates an event in the configured external CalDAV calendar",
          "Writes a local tracked-event record",
        ],
        cost: "No paid API expected; one CalDAV discovery/write sequence",
        recommendedPolicy: "require_approval",
      },
      execute: (input) =>
        Effect.gen(function* () {
          const event = toExtractedEvent(input.event);
          const eventHash = computeEventHash(
            event.title,
            event.startDate,
            event.startTime,
          );
          const existing = yield* getTrackedCalendarEventEffect(eventHash);
          if (existing && existing.status !== "cancelled") {
            return {
              status: "already_exists" as const,
              event: serializeTrackedEvent(existing),
            };
          }
          const session = yield* discoverCaldavSessionEffect(logger);
          const eventUid = `mcp-${createHash("sha256").update(eventHash).digest("hex").slice(0, 32)}@omni-notify`;
          const result = yield* createCalendarEventEffect(
            session,
            event,
            logger,
            eventUid,
          );
          if (result.status === "error") throw new Error(result.message);
          const row: CreatedCalendarEventData = {
            eventHash,
            emailId: "mcp",
            calendarEventId: result.eventUid,
            ...input.event,
            createdAt: yield* Clock.currentTimeMillis,
          };
          yield* recordCreatedEventEffect(row);
          return {
            status: result.status === "already_exists" ? "reconciled" : "created",
            event: serializeTrackedEvent(row),
          };
        }),
    }),
    defineTool({
      name: "calendar_event_update",
      title: "Update Calendar Event",
      description:
        "Patch one active Omni-tracked event and overwrite its external CalDAV representation. Null clears an optional field. This consequential external mutation requires approval.",
      inputSchema: z
        .object({
          eventHash: z.string().min(1).max(1_000),
          changes: calendarEventPatchSchema,
        })
        .strict(),
      outputSchema: z.object({
        status: z.enum(["updated", "unchanged"]),
        event: trackedEventSchema,
      }),
      annotations: annotations(false, true, true, true),
      policy: {
        sideEffects: [
          "Overwrites an event in the configured external CalDAV calendar",
          "Updates local tracked-event identity and content",
        ],
        cost: "No paid API expected; one CalDAV discovery/write sequence when changed",
        recommendedPolicy: "require_approval",
      },
      execute: (input) =>
        Effect.gen(function* () {
          const existing = yield* getTrackedEventOrFailEffect(input.eventHash);
          if (existing.status === "cancelled")
            throw new Error("Cancelled events cannot be updated");
          const clearedChanges = Object.fromEntries(
            Object.entries(input.changes).map(([key, value]) => [
              key,
              value ?? undefined,
            ]),
          );
          const merged = calendarEventInputSchema.parse({
            title: existing.title,
            startDate: existing.startDate,
            startTime: existing.startTime,
            endDate: existing.endDate,
            endTime: existing.endTime,
            allDay: existing.allDay,
            location: existing.location,
            timeZone: existing.timeZone,
            description: existing.description,
            duration: existing.duration,
            reminderMinutes: existing.reminderMinutes,
            recurrence: existing.recurrence,
            ...clearedChanges,
          });
          const event: ExtractedCalendarEvent = { action: "update", ...merged };
          if (!hasEventChanged(existing, event)) {
            return {
              status: "unchanged" as const,
              event: serializeTrackedEvent(existing),
            };
          }
          const newHash = computeEventHash(
            event.title,
            event.startDate,
            event.startTime,
          );
          const priorNewHash =
            newHash === existing.eventHash
              ? undefined
              : yield* getTrackedCalendarEventEffect(newHash);
          if (priorNewHash && priorNewHash.status !== "cancelled") {
            if (priorNewHash.calendarEventId !== existing.calendarEventId) {
              throw new Error("Update would collide with another active tracked event");
            }
            // Reconcile a prior remote/local partial success. The replacement row
            // was persisted before the old row was tombstoned, so no remote write
            // is needed on this retry.
            yield* markEventCancelledEffect(existing.eventHash);
            return {
              status: "updated" as const,
              event: serializeTrackedEvent(priorNewHash),
            };
          }
          const session = yield* discoverCaldavSessionEffect(logger);
          const result = yield* updateCalendarEventEffect(
            session,
            event,
            existing.calendarEventId,
            logger,
          );
          if (result.status === "error") throw new Error(result.message);
          const row: CreatedCalendarEventData = {
            eventHash: newHash,
            emailId: existing.emailId,
            calendarEventId: existing.calendarEventId,
            ...merged,
            createdAt: yield* Clock.currentTimeMillis,
          };
          // Persist the replacement before tombstoning the old identity. If the
          // second write fails, the same request reconciles the two rows above.
          yield* replaceCreatedEventEffect(row, existing.eventHash);
          return { status: "updated" as const, event: serializeTrackedEvent(row) };
        }),
    }),
    defineTool({
      name: "calendar_event_delete",
      title: "Delete Calendar Event",
      description:
        "Delete one Omni-tracked event from the external CalDAV calendar and retain a cancelled local tombstone to prevent accidental recreation. Requires approval.",
      inputSchema: z.object({ eventHash: z.string().min(1).max(1_000) }).strict(),
      outputSchema: z.object({
        status: z.enum(["deleted", "already_deleted"]),
        event: trackedEventSchema,
      }),
      annotations: annotations(false, true, true, true),
      policy: {
        sideEffects: [
          "Deletes an external CalDAV event",
          "Marks the local tracked event cancelled",
        ],
        cost: "No paid API expected; one CalDAV discovery/delete sequence",
        recommendedPolicy: "require_approval",
      },
      execute: (input) =>
        Effect.gen(function* () {
          const existing = yield* getTrackedEventOrFailEffect(input.eventHash);
          if (existing.status === "cancelled") {
            return {
              status: "already_deleted" as const,
              event: serializeTrackedEvent(existing),
            };
          }
          const session = yield* discoverCaldavSessionEffect(logger);
          const result = yield* deleteCalendarEventEffect(
            session,
            existing.calendarEventId,
            logger,
          );
          if (result.status === "error") throw new Error(result.message);
          yield* markEventCancelledEffect(existing.eventHash);
          return {
            status: "deleted" as const,
            event: serializeTrackedEvent({ ...existing, status: "cancelled" }),
          };
        }),
    }),
  ];
}
