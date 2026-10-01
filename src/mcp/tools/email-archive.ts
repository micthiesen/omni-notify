import { Effect } from "effect";
import { z } from "zod";
import {
  ArchiveActionError,
  getArchiveActionEffect,
  queueArchiveActionEffect,
  updateArchiveActionEffect,
  type ArchiveAction,
} from "../../email/archive/persistence.js";
import {
  reconcileClaimedArchiveEffect,
  reconcileClaimedRestoreEffect,
  restoreArchivedActionEffect,
} from "../../email/archive/service.js";
import type { McpRuntime } from "../runtime.js";
import { annotations, defineTool, type McpToolDefinition } from "../tool.js";

const actionIdSchema = z.string().regex(/^[a-f0-9]{64}$/);
const identitySchema = z
  .object({
    folder: z.literal("INBOX"),
    uidValidity: z.string().regex(/^\d+$/).max(24),
    uid: z.number().int().positive(),
    messageId: z
      .string()
      .max(998)
      .regex(/^<[^<>\s\x00-\x1f\x7f]+>$/),
  })
  .strict();

const actionSchema = z.object({
  actionId: actionIdSchema,
  status: z.enum([
    "queued",
    "cancelled",
    "claimed",
    "archived",
    "uncertain",
    "failed",
    "restore_claimed",
    "restored",
    "restore_uncertain",
  ]),
  messageId: z.string(),
  source: identitySchema,
  destination: z
    .object({ folder: z.string(), uidValidity: z.string(), uid: z.number() })
    .nullable(),
  attempts: z.number(),
  nextAttemptAt: z.number(),
  reason: z
    .enum([
      "transport_unavailable",
      "source_unavailable",
      "native_move_unavailable",
      "verification_failed",
      "uncertain",
    ])
    .nullable(),
  createdAt: z.number(),
  updatedAt: z.number(),
});

function serializeAction(action: ArchiveAction) {
  return {
    actionId: action.actionId,
    status: action.status,
    messageId: action.identity.messageId,
    source: action.identity,
    destination: action.destination ?? null,
    attempts: action.attempts,
    nextAttemptAt: action.nextAttemptAt,
    reason: action.reason ?? null,
    createdAt: action.createdAt,
    updatedAt: action.updatedAt,
  };
}

export function createEmailArchiveTools(runtime: McpRuntime): McpToolDefinition[] {
  return [
    defineTool({
      name: "email_archive_queue",
      title: "Queue Exact Inbox Email Archive",
      description:
        "Queue one exact Inbox message for a native move to the uniquely designated Archive mailbox. Supply the Message-ID and origin from a fresh email_get or email_search result. The queue receipt is durable; email_archive_status reports the verified outcome. This does not delete or purge mail.",
      inputSchema: z
        .object({
          idempotencyKey: z.string().trim().min(1).max(200),
          origin: identitySchema.omit({ messageId: true }),
          messageId: identitySchema.shape.messageId,
        })
        .strict(),
      outputSchema: actionSchema,
      annotations: annotations(false, false, true, true),
      policy: {
        sideEffects: ["Queues one exact Inbox message for a native move to Archive"],
        cost: "No paid API; bounded IMAP reads and one native MOVE",
        recommendedPolicy: "require_approval",
      },
      execute: (input) =>
        queueArchiveActionEffect(input.idempotencyKey, {
          ...input.origin,
          messageId: input.messageId,
        }).pipe(Effect.map(serializeAction)),
    }),
    defineTool({
      name: "email_archive_status",
      title: "Check Email Archive Action",
      description:
        "Read a durable archive receipt. Claimed or uncertain operations get read-only reconciliation; this never repeats a MOVE.",
      inputSchema: z.object({ actionId: actionIdSchema }).strict(),
      outputSchema: actionSchema,
      annotations: annotations(true, false, true, true),
      policy: {
        sideEffects: ["May read Inbox and Archive to reconcile an uncertain action"],
        cost: "No paid API; bounded IMAP reads",
        recommendedPolicy: "allow",
      },
      execute: (input) =>
        Effect.gen(function* () {
          let action = yield* getArchiveActionEffect(input.actionId);
          if (!action)
            return yield* new ArchiveActionError({
              message: "Archive action not found",
            });
          const transport = runtime.emailControls.transport;
          if (
            transport &&
            (action.status === "claimed" || action.status === "uncertain")
          )
            action = yield* reconcileClaimedArchiveEffect(action, transport);
          if (
            transport &&
            (action.status === "restore_claimed" ||
              action.status === "restore_uncertain")
          )
            action = yield* reconcileClaimedRestoreEffect(action, transport);
          return serializeAction(action);
        }),
    }),
    defineTool({
      name: "email_archive_cancel",
      title: "Cancel Queued Email Archive",
      description:
        "Cancel an archive action only while it is queued. Once claimed, inspect status instead.",
      inputSchema: z.object({ actionId: actionIdSchema }).strict(),
      outputSchema: actionSchema,
      annotations: annotations(false, false, true, false),
      policy: {
        sideEffects: ["Cancels one queued archive action before any mailbox mutation"],
        cost: "No paid API",
        recommendedPolicy: "allow",
      },
      execute: (input) =>
        Effect.gen(function* () {
          const existing = yield* getArchiveActionEffect(input.actionId);
          if (existing?.status === "cancelled") return serializeAction(existing);
          return serializeAction(
            yield* updateArchiveActionEffect(input.actionId, "queued", "cancelled"),
          );
        }),
    }),
    defineTool({
      name: "email_archive_restore",
      title: "Restore Archived Email to Inbox",
      description:
        "Reverse a verified archive action by moving its exact recorded Archive UID back to Inbox. Refuses changed content, a different Archive mailbox, or an uncertain prior action. Never moves arbitrary mail.",
      inputSchema: z.object({ actionId: actionIdSchema }).strict(),
      outputSchema: actionSchema,
      annotations: annotations(false, false, true, true),
      policy: {
        sideEffects: ["Moves one verified archived email back to Inbox"],
        cost: "No paid API; bounded IMAP reads and one native MOVE",
        recommendedPolicy: "require_approval",
      },
      execute: (input) =>
        Effect.gen(function* () {
          const transport = runtime.emailControls.transport;
          if (!transport)
            return yield* new ArchiveActionError({
              message: "Email monitoring is not active",
            });
          return serializeAction(
            yield* restoreArchivedActionEffect(input.actionId, transport),
          );
        }),
    }),
  ];
}
