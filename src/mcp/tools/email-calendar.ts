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
  AUTO_PASS_SENDERS as CALENDAR_BUILTIN_AUTO_PASS,
  BLACKLISTED_SENDERS as CALENDAR_BUILTIN_BLOCKED,
} from "../../calendar-events/filter/keywords.js";
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
import {
  type EmailActivityData,
  getEmailActivity,
  getRecentEmailActivity,
  KEEP_PER_PIPELINE,
} from "../../email/activity.js";
import { getEmailActivityLogs } from "../../email/activityLogs.js";
import {
  deleteEmailFeedback,
  listEmailFeedback,
  recordEmailFeedback,
} from "../../email/feedback.js";
import { EmailRetryPersistence } from "../../email/retry.js";
import {
  deleteEmailRule,
  listEmailRules,
  normalizeRulePattern,
  upsertEmailRuleChecked,
} from "../../email/senderRules.js";
import type { FetchedEmail } from "../../email/types.js";
import { sendEmailEffect } from "../../emails/send.js";
import {
  CARRIER_SENDER_DOMAINS as PARCEL_BUILTIN_AUTO_PASS,
  BLACKLISTED_SENDERS as PARCEL_BUILTIN_BLOCKED,
} from "../../parcel-tracker/filter/keywords.js";
import config from "../../utils/config.js";
import type { McpRuntime } from "../runtime.js";
import {
  annotations,
  defineTool,
  emptyInputSchema,
  type McpToolDefinition,
  paginate,
  paginationInputShape,
  truncate,
} from "../tool.js";
import { handleEmailThenClearRetryEffect } from "./email-reprocess.js";

const pipelineSchema = z.enum(["ParcelTracker", "CalendarEvents"]);
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

const emailSummarySchema = z.object({
  id: z.string(),
  subject: z.string(),
  from: z.string(),
  receivedAt: z.string(),
  excerpt: z.string(),
  excerptTruncated: z.boolean(),
  attachments: z.array(
    z.object({ name: z.string(), mimeType: z.string(), size: z.number() }),
  ),
});

const activitySchema = z.object({
  activityId: z.string(),
  pipeline: pipelineSchema,
  emailId: z.string(),
  subject: z.string(),
  from: z.string(),
  receivedAt: z.number(),
  processedAt: z.number(),
  outcome: z.string(),
  detail: z.string().nullable(),
  admitReason: z.string().nullable(),
  admitTier: z.string().nullable(),
  costCents: z.number().nullable(),
  items: z.array(z.string()),
});

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

function serializeActivity(
  activity: EmailActivityData,
): z.infer<typeof activitySchema> {
  return {
    activityId: activity.activityId,
    pipeline: activity.pipeline,
    emailId: activity.emailId,
    subject: activity.subject,
    from: activity.from,
    receivedAt: activity.receivedAt,
    processedAt: activity.processedAt,
    outcome: activity.outcome,
    detail: activity.detail ? truncate(activity.detail, 1_000).text : null,
    admitReason: activity.admitReason
      ? truncate(activity.admitReason, 1_000).text
      : null,
    admitTier: activity.admitTier ?? null,
    costCents: activity.costCents ?? null,
    items: (activity.items ?? []).slice(0, 50).map((item) => truncate(item, 500).text),
  };
}

function serializeEmail(email: FetchedEmail, maxExcerptChars: number) {
  const excerpt = truncate(email.textBody, maxExcerptChars);
  return {
    id: email.id,
    subject: truncate(email.subject, 500).text,
    from: truncate(email.from, 500).text,
    receivedAt: email.receivedAt,
    excerpt: excerpt.text,
    excerptTruncated: excerpt.truncated,
    attachments: email.attachments.slice(0, 25).map((attachment) => ({
      name: truncate(attachment.name, 300).text,
      mimeType: truncate(attachment.type, 200).text,
      size: attachment.size,
    })),
  };
}

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

function getActiveEmailRuntime(runtime: McpRuntime) {
  const transport = runtime.emailControls.transport;
  if (!transport) throw new Error("Email monitoring is not active");
  return transport;
}

const getActivityOrThrow = Effect.fn("McpEmail.getActivityOrThrow")(function* (
  activityId: string,
) {
  const activity = yield* getEmailActivity(activityId);
  if (!activity) throw new Error(`Unknown email activity: ${activityId}`);
  return activity;
});

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

function parseDateTime(value: string | undefined): Date | undefined {
  if (!value) return undefined;
  const parsed = new Date(value);
  if (Number.isNaN(parsed.getTime())) throw new Error(`Invalid date-time: ${value}`);
  return parsed;
}

function htmlFromPlainText(text: string): string {
  return `<p>${text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/\n/g, "<br>")}</p>`;
}

function matchesBuiltinBlock(
  pattern: string,
  scope: "parcel" | "calendar" | "both",
): boolean {
  const domain = pattern.startsWith("@")
    ? pattern.slice(1)
    : pattern.includes("@")
      ? null
      : pattern;
  const samples =
    domain === null ? [pattern] : [`probe@${domain}`, `probe@sub.${domain}`];
  const coveredBy = (list: string[]) =>
    samples.every((sample) => list.some((entry) => sample.includes(entry)));
  const parcel = coveredBy(PARCEL_BUILTIN_BLOCKED);
  const calendar = coveredBy(CALENDAR_BUILTIN_BLOCKED);
  return scope === "parcel"
    ? parcel
    : scope === "calendar"
      ? calendar
      : parcel && calendar;
}

export function createEmailCalendarTools(runtime: McpRuntime): McpToolDefinition[] {
  const logger = runtime.logger.extend("MCP:EmailCalendar");

  return [
    defineTool({
      name: "email_search",
      title: "Search Email",
      description:
        "Search the active iCloud IMAP Inbox and Archive with bounded server-side criteria. Returns compact excerpts and attachment metadata, never attachment bytes.",
      inputSchema: z
        .object({
          query: z.string().trim().min(1).max(500).optional(),
          from: z.string().trim().min(1).max(320).optional(),
          to: z.string().trim().min(1).max(320).optional(),
          subject: z.string().trim().min(1).max(500).optional(),
          unread: z.boolean().optional(),
          since: z
            .string()
            .datetime({ offset: true })
            .describe("Inclusive lower bound; IMAP applies day precision")
            .optional(),
          before: z
            .string()
            .datetime({ offset: true })
            .describe("Exclusive upper bound; IMAP applies day precision")
            .optional(),
          folder: z.enum(["inbox", "archive", "all"]).default("all"),
          limit: z.number().int().min(1).max(50).default(20),
          excerptChars: z.number().int().min(0).max(2_000).default(500),
        })
        .strict()
        .refine(
          (value) =>
            Boolean(
              value.query ||
              value.from ||
              value.to ||
              value.subject ||
              value.since ||
              value.before ||
              value.unread !== undefined,
            ),
          "Provide at least one search criterion",
        )
        .refine(
          (value) =>
            !value.since ||
            !value.before ||
            Date.parse(value.before) > Date.parse(value.since),
          "before must be later than since",
        ),
      outputSchema: z.object({ items: z.array(emailSummarySchema), count: z.number() }),
      annotations: annotations(true, false, true, true),
      policy: {
        sideEffects: ["Reads matching messages from the configured personal mailbox"],
        cost: "No paid API; bounded IMAP reads",
        recommendedPolicy: "allow",
      },
      execute: (input) =>
        Effect.gen(function* () {
          const transport = getActiveEmailRuntime(runtime);
          if (!transport.searchEmailsEffect) {
            throw new Error(
              "The active email transport does not support mailbox search",
            );
          }
          const emails = yield* transport.searchEmailsEffect({
            query: input.query,
            from: input.from,
            to: input.to,
            subject: input.subject,
            unread: input.unread,
            since: parseDateTime(input.since),
            before: parseDateTime(input.before),
            folder: input.folder,
            limit: input.limit,
          });
          return {
            items: emails.map((email) => serializeEmail(email, input.excerptChars)),
            count: emails.length,
          };
        }),
    }),

    defineTool({
      name: "email_get",
      title: "Get Email",
      description:
        "Fetch one email by the stable identifier returned by email_search or email activity. Body text and attachment metadata are bounded; attachment bytes and credentials are never returned.",
      inputSchema: z
        .object({
          emailId: z.string().min(1).max(1_000),
          bodyChars: z.number().int().min(0).max(20_000).default(8_000),
        })
        .strict(),
      outputSchema: z.object({ email: emailSummarySchema }),
      annotations: annotations(true, false, true, true),
      policy: {
        sideEffects: ["Reads one message from the configured personal mailbox"],
        cost: "No paid API; one bounded IMAP lookup",
        recommendedPolicy: "allow",
      },
      execute: (input) =>
        Effect.gen(function* () {
          const email = yield* getActiveEmailRuntime(runtime).fetchEmailByIdEffect(
            input.emailId,
          );
          if (!email)
            throw new Error("Email no longer exists in the monitored mailbox");
          return { email: serializeEmail(email, input.bodyChars) };
        }),
    }),

    defineTool({
      name: "email_health",
      title: "Inspect Email and Calendar Health",
      description:
        "Report whether email monitoring, SMTP sending, and the active CalDAV provider are configured. This is a local configuration/runtime check and does not reveal credentials or probe external services.",
      inputSchema: emptyInputSchema,
      outputSchema: z.object({
        monitoring: z.object({
          active: z.boolean(),
          transport: z.string().nullable(),
          pipelines: z.array(z.string()),
          searchAvailable: z.boolean(),
        }),
        smtp: z.object({ configured: z.boolean(), configuredFrom: z.boolean() }),
        caldav: z.object({ configured: z.boolean(), provider: z.string().nullable() }),
      }),
      annotations: annotations(true, false, true, false),
      policy: {
        sideEffects: [],
        cost: "None",
        recommendedPolicy: "allow",
      },
      execute: () =>
        Effect.sync(() => {
          const transport = runtime.emailControls.transport;
          const provider = getCaldavProvider();
          return {
            monitoring: {
              active: Boolean(transport),
              transport: transport?.name ?? null,
              pipelines: [...(runtime.emailControls.handlers?.keys() ?? [])].sort(),
              searchAvailable: Boolean(transport?.searchEmailsEffect),
            },
            smtp: {
              configured: Boolean(
                config.SMTP_HOST && config.SMTP_USER && config.SMTP_PASS,
              ),
              configuredFrom: Boolean(config.EMAIL_FROM),
            },
            caldav: { configured: Boolean(provider), provider: provider ?? null },
          };
        }),
    }),

    defineTool({
      name: "email_activity_list",
      title: "List Email Pipeline Activity",
      description:
        "List bounded, newest-first outcomes from the parcel and calendar email pipelines, including compact result details and attributed model cost.",
      inputSchema: z
        .object({
          ...paginationInputShape,
          pipeline: pipelineSchema.optional(),
        })
        .strict(),
      outputSchema: z.object({
        items: z.array(activitySchema),
        nextCursor: z.number().nullable(),
        total: z.number(),
      }),
      annotations: annotations(true, false, true, false),
      policy: { sideEffects: [], cost: "None", recommendedPolicy: "allow" },
      execute: (input) =>
        Effect.gen(function* () {
          const activities = yield* getRecentEmailActivity(
            input.pipeline,
            KEEP_PER_PIPELINE * 2,
          );
          return paginate(activities.map(serializeActivity), input.cursor, input.limit);
        }),
    }),

    defineTool({
      name: "email_activity_get",
      title: "Get Email Pipeline Activity",
      description:
        "Get one pipeline outcome plus a bounded tail of its captured processing logs. Log messages are returned compactly and may be truncated.",
      inputSchema: z
        .object({
          activityId: z.string().min(1).max(1_200),
          logLimit: z.number().int().min(0).max(500).default(100),
        })
        .strict(),
      outputSchema: z.object({
        activity: activitySchema,
        logs: z.array(
          z.object({
            timestamp: z.number(),
            level: z.string(),
            logger: z.string(),
            message: z.string(),
          }),
        ),
        dropped: z.number(),
        logsTruncated: z.boolean(),
      }),
      annotations: annotations(true, false, true, false),
      policy: { sideEffects: [], cost: "None", recommendedPolicy: "allow" },
      execute: (input) =>
        Effect.gen(function* () {
          const activity = yield* getActivityOrThrow(input.activityId);
          const stored = yield* getEmailActivityLogs(input.activityId);
          const lines = stored?.lines ?? [];
          const selected = input.logLimit === 0 ? [] : lines.slice(-input.logLimit);
          return {
            activity: serializeActivity(activity),
            logs: selected.map((line) => ({
              timestamp: line.t,
              level: line.level,
              logger: line.logger,
              message: truncate(line.msg, 4_000).text,
            })),
            dropped: stored?.dropped ?? 0,
            logsTruncated: selected.length < lines.length,
          };
        }),
    }),

    defineTool({
      name: "email_reprocess",
      title: "Reprocess Email",
      description:
        "Re-fetch an email and rerun its recorded parcel or calendar pipeline. This may invoke priced models and external Parcel, CalDAV, or notification services; dedup gates reduce but do not eliminate consequential effects.",
      inputSchema: z.object({ activityId: z.string().min(1).max(1_200) }).strict(),
      outputSchema: z.object({ activity: activitySchema }),
      annotations: annotations(false, false, false, true),
      policy: {
        sideEffects: [
          "Clears a queued retry for the activity",
          "Reruns extraction and may submit a parcel, mutate CalDAV, or send a notification",
        ],
        cost: "May incur configured LLM and third-party workflow costs",
        recommendedPolicy: "require_approval",
      },
      execute: (input) =>
        Effect.gen(function* () {
          const activity = yield* getActivityOrThrow(input.activityId);
          const transport = getActiveEmailRuntime(runtime);
          const handler = runtime.emailControls.handlers?.get(activity.pipeline);
          if (!handler)
            throw new Error(`Email pipeline is not active: ${activity.pipeline}`);
          const email = yield* transport.fetchEmailByIdEffect(activity.emailId);
          if (!email)
            throw new Error("Email no longer exists in the monitored mailbox");
          yield* handleEmailThenClearRetryEffect(handler, email, () =>
            EmailRetryPersistence.clear(activity.pipeline, activity.emailId),
          );
          return {
            activity: serializeActivity(
              (yield* getEmailActivity(activity.activityId)) ?? activity,
            ),
          };
        }),
    }),

    defineTool({
      name: "email_rules_list",
      title: "List Email Sender Rules",
      description:
        "List user-managed sender allow/block rules and the read-only built-in filter lists consulted by the parcel and calendar pipelines.",
      inputSchema: emptyInputSchema,
      outputSchema: z.object({
        rules: z.array(
          z.object({
            ruleId: z.string(),
            pattern: z.string(),
            scope: z.enum(["parcel", "calendar", "both"]),
            verdict: z.enum(["block", "allow"]),
            createdAt: z.number(),
          }),
        ),
        builtin: z.object({
          parcel: z.object({
            blocked: z.array(z.string()),
            autoPass: z.array(z.string()),
          }),
          calendar: z.object({
            blocked: z.array(z.string()),
            autoPass: z.array(z.string()),
          }),
        }),
      }),
      annotations: annotations(true, false, true, false),
      policy: { sideEffects: [], cost: "None", recommendedPolicy: "allow" },
      execute: () =>
        Effect.gen(function* () {
          return {
            rules: yield* listEmailRules(),
            builtin: {
              parcel: {
                blocked: [...PARCEL_BUILTIN_BLOCKED],
                autoPass: [...PARCEL_BUILTIN_AUTO_PASS],
              },
              calendar: {
                blocked: [...CALENDAR_BUILTIN_BLOCKED],
                autoPass: [...CALENDAR_BUILTIN_AUTO_PASS],
              },
            },
          };
        }),
    }),

    defineTool({
      name: "email_rules_upsert",
      title: "Add or Update Email Sender Rule",
      description:
        "Add or replace a normalized sender allow/block rule for parcel processing, calendar processing, or both. This changes how future personal email is handled.",
      inputSchema: z
        .object({
          pattern: z.string().trim().min(1).max(200),
          scope: z.enum(["parcel", "calendar", "both"]),
          verdict: z.enum(["block", "allow"]),
        })
        .strict(),
      outputSchema: z.object({
        status: z.enum(["created", "merged", "exists", "builtin"]),
        rule: z
          .object({
            ruleId: z.string(),
            pattern: z.string(),
            scope: z.enum(["parcel", "calendar", "both"]),
            verdict: z.enum(["block", "allow"]),
            createdAt: z.number(),
          })
          .nullable(),
      }),
      annotations: annotations(false, false, true, false),
      policy: {
        sideEffects: ["Changes persistent sender filtering for future email workflows"],
        cost: "None",
        recommendedPolicy: "allow",
      },
      execute: (input) =>
        Effect.gen(function* () {
          const pattern = normalizeRulePattern(input.pattern);
          if (input.verdict === "block" && matchesBuiltinBlock(pattern, input.scope)) {
            return { status: "builtin" as const, rule: null };
          }
          const result = yield* upsertEmailRuleChecked({ ...input, pattern });
          return {
            status: result.alreadyExists
              ? ("exists" as const)
              : result.merged
                ? ("merged" as const)
                : ("created" as const),
            rule: result.rule,
          };
        }),
    }),

    defineTool({
      name: "email_rules_delete",
      title: "Delete Email Sender Rule",
      description:
        "Delete one user-managed sender rule by ruleId. Built-in rules cannot be deleted through MCP.",
      inputSchema: z.object({ ruleId: z.string().min(1).max(500) }).strict(),
      outputSchema: z.object({ deleted: z.boolean() }),
      annotations: annotations(false, true, true, false),
      policy: {
        sideEffects: [
          "Deletes a persistent sender rule and changes future email filtering",
        ],
        cost: "None",
        recommendedPolicy: "require_approval",
      },
      execute: (input) =>
        deleteEmailRule(input.ruleId).pipe(Effect.map((deleted) => ({ deleted }))),
    }),

    defineTool({
      name: "email_feedback_list",
      title: "List Email Feedback",
      description:
        "List explicit corrections used by the email relevance triage prompts, newest first.",
      inputSchema: z
        .object({
          pipeline: pipelineSchema.optional(),
          limit: z.number().int().min(1).max(100).default(50),
        })
        .strict(),
      outputSchema: z.object({
        items: z.array(
          z.object({
            activityId: z.string(),
            pipeline: pipelineSchema,
            emailId: z.string(),
            subject: z.string(),
            from: z.string(),
            verdict: z.enum(["not_relevant", "missed"]),
            note: z.string().nullable(),
            createdAt: z.number(),
          }),
        ),
      }),
      annotations: annotations(true, false, true, false),
      policy: { sideEffects: [], cost: "None", recommendedPolicy: "allow" },
      execute: (input) =>
        Effect.gen(function* () {
          return {
            items: (yield* listEmailFeedback(input.pipeline, input.limit)).map(
              (feedback) => ({ ...feedback, note: feedback.note ?? null }),
            ),
          };
        }),
    }),

    defineTool({
      name: "email_feedback_set",
      title: "Set Email Feedback",
      description:
        "Set or clear a not-relevant/missed correction for an existing email activity. Corrections influence future model triage decisions.",
      inputSchema: z
        .object({
          activityId: z.string().min(1).max(1_200),
          verdict: z.enum(["not_relevant", "missed"]).nullable(),
          note: z.string().trim().max(500).optional(),
        })
        .strict(),
      outputSchema: z.object({
        feedback: z
          .object({
            activityId: z.string(),
            pipeline: pipelineSchema,
            emailId: z.string(),
            subject: z.string(),
            from: z.string(),
            verdict: z.enum(["not_relevant", "missed"]),
            note: z.string().nullable(),
            createdAt: z.number(),
          })
          .nullable(),
      }),
      annotations: annotations(false, true, false, false),
      policy: {
        sideEffects: [
          "Changes persistent correction data injected into future triage prompts",
        ],
        cost: "None",
        recommendedPolicy: "allow",
      },
      execute: (input) =>
        Effect.gen(function* () {
          const activity = yield* getActivityOrThrow(input.activityId);
          if (input.verdict === null) {
            yield* deleteEmailFeedback(activity.activityId);
            return { feedback: null };
          }
          const feedback = yield* recordEmailFeedback({
            pipeline: activity.pipeline,
            emailId: activity.emailId,
            subject: activity.subject,
            from: activity.from,
            verdict: input.verdict,
            note: input.note || undefined,
          });
          return { feedback: { ...feedback, note: feedback.note ?? null } };
        }),
    }),

    defineTool({
      name: "email_retry_list",
      title: "List Email Retries",
      description:
        "List bounded persisted retries for transient parcel or calendar pipeline failures, ordered by next attempt.",
      inputSchema: z
        .object({ ...paginationInputShape, pipeline: pipelineSchema.optional() })
        .strict(),
      outputSchema: z.object({
        items: z.array(
          z.object({
            retryKey: z.string(),
            pipeline: z.string(),
            emailId: z.string(),
            reason: z.string(),
            attempts: z.number(),
            nextAttemptAt: z.number(),
            createdAt: z.number(),
          }),
        ),
        nextCursor: z.number().nullable(),
        total: z.number(),
      }),
      annotations: annotations(true, false, true, false),
      policy: { sideEffects: [], cost: "None", recommendedPolicy: "allow" },
      execute: (input) =>
        Effect.gen(function* () {
          const retries = (yield* EmailRetryPersistence.getAll())
            .filter((retry) => !input.pipeline || retry.pipeline === input.pipeline)
            .sort((a, b) => a.nextAttemptAt - b.nextAttemptAt)
            .map((retry) => ({ ...retry, reason: truncate(retry.reason, 1_000).text }));
          return paginate(retries, input.cursor, input.limit);
        }),
    }),

    defineTool({
      name: "email_retry_clear",
      title: "Clear Email Retry",
      description:
        "Remove a persisted retry for one pipeline/email pair. This can prevent an otherwise scheduled external workflow from completing.",
      inputSchema: z
        .object({ pipeline: pipelineSchema, emailId: z.string().min(1).max(1_000) })
        .strict(),
      outputSchema: z.object({ cleared: z.boolean() }),
      annotations: annotations(false, true, true, false),
      policy: {
        sideEffects: ["Deletes a pending persistent retry"],
        cost: "None",
        recommendedPolicy: "require_approval",
      },
      execute: (input) =>
        Effect.gen(function* () {
          const retryKey = `${input.pipeline}#${input.emailId}`;
          const existed = Boolean(yield* EmailRetryPersistence.get(retryKey));
          yield* EmailRetryPersistence.clear(input.pipeline, input.emailId);
          return { cleared: existed };
        }),
    }),

    defineTool({
      name: "email_send",
      title: "Send Email",
      description:
        "Send one plain-text email through Omni's configured SMTP account and configured sender address. This is an external communication and always requires Executor approval.",
      inputSchema: z
        .object({
          to: z.string().trim().email().max(320),
          subject: z.string().trim().min(1).max(200),
          text: z.string().min(1).max(20_000),
        })
        .strict(),
      outputSchema: z.object({
        sent: z.boolean(),
        to: z.string(),
        subject: z.string(),
      }),
      annotations: annotations(false, false, false, true),
      policy: {
        sideEffects: ["Sends an external email to the specified recipient"],
        cost: "No per-call paid API expected; consumes SMTP provider quota",
        recommendedPolicy: "require_approval",
      },
      execute: (input) =>
        Effect.gen(function* () {
          if (!config.EMAIL_FROM)
            throw new Error("SMTP sender address is not configured");
          if (!config.SMTP_HOST || !config.SMTP_USER || !config.SMTP_PASS) {
            throw new Error("SMTP is not configured");
          }
          const sent = yield* sendEmailEffect({
            to: input.to,
            from: config.EMAIL_FROM,
            subject: input.subject,
            text: input.text,
            html: htmlFromPlainText(input.text),
          });
          if (!sent) throw new Error("SMTP delivery failed");
          return { sent: true, to: input.to, subject: input.subject };
        }),
    }),

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
