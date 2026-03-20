import { z } from "zod";

export const calendarEventExtractionSchema = z.object({
  events: z.array(
    z.object({
      title: z
        .string()
        .describe(
          "Short event title prefixed with a relevant emoji in Title Case (e.g. '🦷 Dentist Appointment', '✈️ Flight to Vancouver')",
        ),
      startDate: z.string().describe("ISO 8601 date, e.g. 2026-03-20"),
      endDate: z
        .string()
        .optional()
        .describe(
          "ISO 8601 end date if different from startDate (e.g. multi-day hotel stay). Omit for single-day events",
        ),
      startTime: z
        .string()
        .optional()
        .describe("24-hour time, e.g. 14:30. Omit for all-day events"),
      endTime: z
        .string()
        .optional()
        .describe("24-hour end time, e.g. 16:00. Omit if unknown"),
      duration: z
        .string()
        .optional()
        .describe("ISO 8601 duration if end time not known, e.g. PT1H30M"),
      location: z.string().optional().describe("Event location or venue address"),
      description: z
        .string()
        .optional()
        .describe("Brief notes or details about the event"),
      timeZone: z
        .string()
        .optional()
        .describe("IANA timezone, e.g. America/Toronto. Omit to use default"),
      allDay: z
        .boolean()
        .describe("True if this is an all-day event with no specific time"),
    }),
  ),
});

export type CalendarEventExtraction = z.infer<typeof calendarEventExtractionSchema>;
