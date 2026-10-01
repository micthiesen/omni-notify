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
  advanceCopyActionEffect,
  withArchiveActionWorkflowEffect,
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
    "copy_claimed",
    "copy_verified",
    "delete_claimed",
    "expunge_claimed",
    "copied_source_retained",
    "restore_copy_claimed",
    "restore_copy_verified",
    "restore_delete_claimed",
    "restore_expunge_claimed",
    "restore_copied_source_retained",
  ]),
  messageId: z.string(),
  source: identitySchema,
  destination: z
    .object({ folder: z.string(), uidValidity: z.string(), uid: z.number() })
    .nullable(),
  restoredLocation: z
    .object({ folder: z.string(), uidValidity: z.string(), uid: z.number() })
    .nullable(),
  attempts: z.number(),
  nextAttemptAt: z.number(),
  reason: z
    .enum([
      "transport_unavailable",
      "source_unavailable",
      "native_move_unavailable",
      "safe_move_unavailable",
      "verification_failed",
      "uncertain",
      "copy_uncertain",
      "copied_source_retained",
      "copied_source_deleted",
      "source_mark_uncertain",
      "source_expunge_uncertain",
    ])
    .nullable(),
  createdAt: z.number(),
  updatedAt: z.number(),
});

export function serializeAction(action: ArchiveAction) {
  return {
    actionId: action.actionId,
    status: action.status,
    messageId: action.identity.messageId,
    source: action.identity,
    destination: action.destination ?? null,
    restoredLocation: action.restoredLocation ?? null,
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
        "Queue one exact Inbox message for Archive. Uses native MOVE when available; otherwise verifies UIDPLUS COPY, then permanently removes the exact Inbox source with UID EXPUNGE. Supply Message-ID and origin from fresh email_get or email_search. Check durable status for outcome.",
      inputSchema: z
        .object({
          idempotencyKey: z.string().trim().min(1).max(200),
          origin: identitySchema.omit({ messageId: true }),
          messageId: identitySchema.shape.messageId,
        })
        .strict(),
      outputSchema: actionSchema,
      annotations: annotations(false, true, true, true),
      policy: {
        sideEffects: [
          "Queues one exact Inbox message for native MOVE or scoped UIDPLUS COPY and source removal",
        ],
        cost: "No paid API; bounded IMAP reads and one scoped mailbox move",
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
        "Read a durable archive receipt and reconcile claimed outcomes using mailbox reads. This tool never writes mail or repeats a mutation.",
      inputSchema: z.object({ actionId: actionIdSchema }).strict(),
      outputSchema: actionSchema,
      annotations: annotations(true, false, true, true),
      policy: {
        sideEffects: ["May read Inbox and Archive to reconcile an uncertain action"],
        cost: "No paid API; bounded IMAP reads",
        recommendedPolicy: "allow",
      },
      execute: (input) =>
        withArchiveActionWorkflowEffect(
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
            if (
              transport &&
              [
                "copy_claimed",
                "copy_verified",
                "delete_claimed",
                "expunge_claimed",
                "copied_source_retained",
              ].includes(action.status)
            )
              action = yield* advanceCopyActionEffect(
                action,
                transport,
                false,
                undefined,
                false,
              );
            if (
              transport &&
              [
                "restore_copy_claimed",
                "restore_copy_verified",
                "restore_delete_claimed",
                "restore_expunge_claimed",
                "restore_copied_source_retained",
              ].includes(action.status)
            )
              action = yield* advanceCopyActionEffect(
                action,
                transport,
                true,
                undefined,
                false,
              );
            return serializeAction(action);
          }),
        ),
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
        "Restore only this action's recorded Archive UID to Inbox. Uses native MOVE or verified UIDPLUS COPY followed by permanent exact-source UID EXPUNGE. Refuses changed content, a different Archive mailbox, or an uncertain action.",
      inputSchema: z.object({ actionId: actionIdSchema }).strict(),
      outputSchema: actionSchema,
      annotations: annotations(false, true, true, true),
      policy: {
        sideEffects: [
          "Restores one exact recorded Archive UID using native MOVE or verified COPY and exact-source UID EXPUNGE",
        ],
        cost: "No paid API; bounded IMAP reads and one scoped mailbox move",
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
