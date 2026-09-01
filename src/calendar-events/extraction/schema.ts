import { z } from "zod";
import { Schema } from "effect";

// OpenAI strict structured outputs require every object property to appear in
// `required`. Represent optional values as nullable in JSON, then normalize
// null back to undefined so the rest of the calendar pipeline keeps its
// existing optional-field contract.
const optional = <T extends z.ZodType>(schema: T) =>
  schema.nullable().transform((value) => value ?? undefined);

export const calendarEventExtractionSchema = z.object({
  events: z.array(
    z.object({
      action: z
        .enum(["create", "cancel", "update"])
        .describe(
          "'create' for new events, 'cancel' for cancelled events, 'update' for rescheduled/modified events",
        ),
      eventId: optional(z.string()).describe(
        "For 'cancel' and 'update' only: the id shown in square brackets next to the existing event this action refers to, WITHOUT the brackets (for [evt_2], use evt_2). Copy it exactly from the existing events list. Use null for 'create'",
      ),
      title: z
        .string()
        .describe(
          "Short event title prefixed with a relevant emoji in Title Case (e.g. '🦷 Dentist Appointment', '✈️ Flight to Vancouver')",
        ),
      startDate: z.string().describe("ISO 8601 date, e.g. 2026-03-20"),
      endDate: optional(z.string()).describe(
        "ISO 8601 end date if different from startDate (e.g. multi-day hotel stay). Omit for single-day events",
      ),
      startTime: optional(z.string()).describe(
        "24-hour time, e.g. 14:30. Omit for all-day events",
      ),
      endTime: optional(z.string()).describe(
        "24-hour end time, e.g. 16:00. Omit if unknown",
      ),
      duration: optional(z.string()).describe(
        "ISO 8601 duration if end time not known, e.g. PT1H30M",
      ),
      location: optional(z.string()).describe("Event location or venue address"),
      description: optional(z.string()).describe(
        "Brief notes or details about the event",
      ),
      timeZone: optional(z.string()).describe(
        "IANA timezone, e.g. America/Toronto. Omit to use default",
      ),
      recurrence: optional(
        z.object({
          frequency: z.enum(["daily", "weekly", "monthly"]),
          until: z
            .string()
            .describe(
              "ISO 8601 date of the last occurrence (inclusive), e.g. 2026-07-13",
            ),
        }),
      ).describe(
        "For events repeating on a fixed pattern. A notice like 'daily 9:00-16:00 from Jul 6 to Jul 13' is ONE event on the first day with recurrence { frequency: 'daily', until: '2026-07-13' }. Null/omit for one-off events",
      ),
      allDay: z
        .boolean()
        .describe("True if this is an all-day event with no specific time"),
      reminderMinutes: optional(z.number()).describe(
        "Minutes before the event to send a reminder. Use for events that benefit from advance preparation (e.g. 720 for a water shutoff the night before, 1440 for a flight the day before, 60 for appointments). Omit to use the default 30-minute reminder",
      ),
    }),
  ),
});

type StrictExtractedCalendarEvent = z.infer<
  typeof calendarEventExtractionSchema
>["events"][number];

type OptionalEventField =
  | "eventId"
  | "endDate"
  | "startTime"
  | "endTime"
  | "duration"
  | "location"
  | "description"
  | "timeZone"
  | "recurrence"
  | "reminderMinutes";

/** Normalized application shape after the strict nullable JSON is parsed. */
export type ExtractedCalendarEvent = Omit<
  StrictExtractedCalendarEvent,
  OptionalEventField
> & {
  [K in OptionalEventField]?:
    | StrictExtractedCalendarEvent[K]
    | (K extends "recurrence" ? null : never);
};

export interface CalendarEventExtraction {
  events: ExtractedCalendarEvent[];
}

const OptionalString = Schema.optional(Schema.String);
export const CalendarEventExtractionEffectSchema = Schema.Struct({
  events: Schema.Array(
    Schema.Struct({
      action: Schema.Literals(["create", "cancel", "update"]),
      eventId: OptionalString,
      title: Schema.String,
      startDate: Schema.String,
      endDate: OptionalString,
      startTime: OptionalString,
      endTime: OptionalString,
      duration: OptionalString,
      location: OptionalString,
      description: OptionalString,
      timeZone: OptionalString,
      recurrence: Schema.optional(
        Schema.Struct({
          frequency: Schema.Literals(["daily", "weekly", "monthly"]),
          until: Schema.String,
        }),
      ),
      allDay: Schema.Boolean,
      reminderMinutes: Schema.optional(Schema.Number),
    }),
  ),
});
