import { createHash } from "node:crypto";
import { decodeDoc, Docstore } from "@micthiesen/mitools/docstore";
import { Clock, Data, Effect, Option, Schema } from "effect";
import { z } from "zod";
import type { EmailDraftInput } from "../../email/types.js";
import { getComposeEmailConfiguration } from "../../emails/client.js";
import { sendComposedEmailEffect } from "../../emails/send.js";
import { prepareComposedEmailEffect } from "../../emails/mime.js";
import type { McpRuntime } from "../runtime.js";
import { annotations, defineTool, type McpToolDefinition } from "../tool.js";

const recipient = z.string().trim().email().max(320);
const recipientList = z.array(recipient).min(1).max(20);
const addresses = z
  .union([recipient, recipientList])
  .transform((value) =>
    (Array.isArray(value) ? value : [value]).map((address) => address.trim()),
  );
const messageIdSchema = z
  .string()
  .trim()
  .min(3)
  .max(998)
  .regex(/^<[^<>\s]+@[^<>\s]+>$/, "Expected an RFC Message-ID in angle brackets");

const composeFields = {
  idempotencyKey: z.string().trim().min(1).max(200),
  to: addresses,
  cc: z.array(recipient).max(20).optional(),
  bcc: z.array(recipient).max(20).optional(),
  subject: z
    .string()
    .trim()
    .min(1)
    .max(200)
    .regex(/^[^\r\n]*$/, "Subject must not contain line breaks"),
  text: z.string().min(1).max(20_000),
  inReplyTo: messageIdSchema.optional(),
  references: z.array(messageIdSchema).max(20).optional(),
};

export class EmailComposeError extends Data.TaggedError("EmailComposeError")<{
  readonly message: string;
  readonly cause?: unknown;
}> {}

const PreparedMessageSchema = Schema.Struct({
  from: Schema.String,
  date: Schema.String,
  wire: Schema.String,
  content: Schema.String,
});
type PreparedMessage = typeof PreparedMessageSchema.Type;
const SentCopySchema = Schema.Literals(["pending", "uncertain", "verified"]);
const StoredAttemptSchema = Schema.Struct({
  fingerprint: Schema.String,
  status: Schema.Literals(["pending", "succeeded", "failed"]),
  result: Schema.optional(
    Schema.Struct({
      sent: Schema.Boolean,
      messageId: Schema.String,
    }),
  ),
  prepared: Schema.optional(PreparedMessageSchema),
  sentCopy: Schema.optional(SentCopySchema),
  updatedAt: Schema.Number,
});
type StoredAttempt = typeof StoredAttemptSchema.Type;

type Reservation =
  | { kind: "reserved" }
  | { kind: "succeeded"; messageId: string }
  | { kind: "pending" | "failed" }
  | { kind: "mismatch" };

const keyFor = (kind: "draft" | "send", key: string) =>
  `email-compose:${kind}:${createHash("sha256").update(key).digest("hex")}`;
const fingerprintFor = (input: unknown) =>
  createHash("sha256").update(JSON.stringify(input)).digest("hex");

function reserveEffect(
  kind: "draft" | "send",
  idempotencyKey: string,
  fingerprint: string,
  prepared?: PreparedMessage,
): Effect.Effect<Reservation, EmailComposeError, Docstore> {
  return Effect.gen(function* () {
    const docstore = yield* Docstore;
    const now = yield* Clock.currentTimeMillis;
    return yield* docstore.transaction(`reserve email ${kind}`, (tx): Reservation => {
      const pk = keyFor(kind, idempotencyKey);
      const stored = tx.getRawRow(pk, now);
      const prior = stored
        ? Schema.decodeUnknownSync(StoredAttemptSchema)(decodeDoc<unknown>(stored.data))
        : undefined;
      if (prior) {
        if (prior.fingerprint !== fingerprint) return { kind: "mismatch" };
        if (prior.status === "succeeded" && prior.result?.sent) {
          return { kind: "succeeded", messageId: prior.result.messageId };
        }
        if (prior.status === "succeeded") return { kind: "failed" };
        return { kind: prior.status };
      }
      const row: StoredAttempt = {
        fingerprint,
        status: "pending",
        updatedAt: now,
        ...(prepared ? { prepared, sentCopy: "pending" as const } : {}),
      };
      tx.upsertDoc(pk, row, { entity: `email-compose-${kind}` }, now);
      return { kind: "reserved" };
    });
  }).pipe(
    Effect.mapError(
      (cause) =>
        new EmailComposeError({
          message: `Could not reserve email ${kind}`,
          cause,
        }),
    ),
  );
}

function completeEffect(
  kind: "draft" | "send",
  idempotencyKey: string,
  fingerprint: string,
  sent: boolean,
  messageId: string,
): Effect.Effect<void, EmailComposeError, Docstore> {
  return Effect.gen(function* () {
    const docstore = yield* Docstore;
    const now = yield* Clock.currentTimeMillis;
    yield* docstore.transaction(`complete email ${kind}`, (tx) => {
      const pk = keyFor(kind, idempotencyKey);
      const stored = tx.getRawRow(pk, now);
      if (!stored) throw new Error("Email compose reservation disappeared");
      const prior = Schema.decodeUnknownSync(StoredAttemptSchema)(
        decodeDoc<unknown>(stored.data),
      );
      if (prior.fingerprint !== fingerprint || prior.status !== "pending") {
        throw new Error("Email compose reservation changed unexpectedly");
      }
      tx.upsertDoc(
        pk,
        {
          ...prior,
          status: sent ? "succeeded" : "failed",
          result: { sent, messageId },
          updatedAt: now,
        },
        { entity: `email-compose-${kind}` },
        now,
      );
    });
  }).pipe(
    Effect.mapError(
      (cause) =>
        new EmailComposeError({
          message: `Could not persist email ${kind} outcome; do not repeat with a new key`,
          cause,
        }),
    ),
  );
}

function readAttemptEffect(idempotencyKey: string) {
  return Effect.gen(function* () {
    const docstore = yield* Docstore;
    const row = yield* docstore.getRawRow(keyFor("send", idempotencyKey));
    if (Option.isNone(row)) return undefined;
    return yield* Schema.decodeUnknownEffect(StoredAttemptSchema)(
      decodeDoc<unknown>(row.value.data),
    );
  }).pipe(
    Effect.mapError(
      (cause) =>
        new EmailComposeError({ message: "Could not read send receipt", cause }),
    ),
  );
}

/** Claim APPEND durably; a crash or lost response permits read-only reconciliation only. */
function saveSentEffect(runtime: McpRuntime, idempotencyKey: string) {
  return Effect.gen(function* () {
    const attempt = yield* readAttemptEffect(idempotencyKey);
    if (!attempt?.result?.sent || attempt.status !== "succeeded")
      return yield* new EmailComposeError({
        message:
          "SMTP acceptance is not confirmed; Sent copy repair cannot send or assume delivery",
      });
    if (attempt.sentCopy === "verified") return "verified" as const;
    if (!attempt.prepared) return "legacy-unavailable" as const;
    const transport = runtime.emailControls.transport;
    if (!transport?.saveSentCopyEffect) return attempt.sentCopy ?? "pending";
    const docstore = yield* Docstore;
    const now = yield* Clock.currentTimeMillis;
    const beforeAppend = docstore.transaction("claim Sent APPEND", (tx) => {
      const row = tx.getRawRow(keyFor("send", idempotencyKey), now);
      if (!row) throw new Error("Send receipt disappeared");
      const prior = Schema.decodeUnknownSync(StoredAttemptSchema)(
        decodeDoc<unknown>(row.data),
      );
      if (prior.sentCopy !== "pending") return false;
      tx.upsertDoc(
        keyFor("send", idempotencyKey),
        { ...prior, sentCopy: "uncertain" },
        { entity: "email-compose-send" },
        now,
      );
      return true;
    });
    const copied = yield* transport
      .saveSentCopyEffect(
        {
          messageId: attempt.result.messageId,
          content: Buffer.from(attempt.prepared.content, "base64"),
          internalDate: new Date(attempt.prepared.date),
        },
        { allowAppend: attempt.sentCopy === "pending", beforeAppend },
      )
      .pipe(Effect.result);
    if (copied._tag === "Failure")
      return (yield* readAttemptEffect(idempotencyKey))?.sentCopy ?? "pending";
    yield* docstore.transaction("verify Sent copy receipt", (tx) => {
      const row = tx.getRawRow(keyFor("send", idempotencyKey), now);
      if (!row) throw new Error("Send receipt disappeared");
      const prior = Schema.decodeUnknownSync(StoredAttemptSchema)(
        decodeDoc<unknown>(row.data),
      );
      tx.upsertDoc(
        keyFor("send", idempotencyKey),
        { ...prior, sentCopy: "verified" },
        { entity: "email-compose-send" },
        now,
      );
    });
    return "verified" as const;
  }).pipe(
    Effect.mapError(
      (cause) =>
        new EmailComposeError({
          message: "SMTP was accepted but Sent copy persistence failed; do not resend",
          cause,
        }),
    ),
  );
}

function reservationFailure(
  action: string,
  reservation: Reservation,
): EmailComposeError {
  switch (reservation.kind) {
    case "mismatch":
      return new EmailComposeError({
        message: `Idempotency key already belongs to different ${action} content`,
      });
    case "pending":
      return new EmailComposeError({
        message: `A prior ${action} attempt has an uncertain outcome; it will not be retried automatically. Do not retry with a new key without checking delivery.`,
      });
    case "failed":
      return new EmailComposeError({
        message: `A prior ${action} attempt was not confirmed and will not be retried automatically. Do not retry with a new key without checking delivery.`,
      });
    case "succeeded":
      return new EmailComposeError({
        message: `Unexpected successful ${action} reservation state`,
      });
    case "reserved":
      return new EmailComposeError({
        message: `Unexpected reservation state for ${action}`,
      });
  }
}

export function createEmailComposeTools(runtime: McpRuntime): McpToolDefinition[] {
  const draftSchema = z.object(composeFields).strict();
  const sendSchema = z.object(composeFields).strict();

  return [
    defineTool({
      name: "email_draft_create",
      title: "Create Email Draft",
      description:
        "Save a plain-text draft in the server-designated Drafts mailbox. Reusing an idempotency key with different content is rejected.",
      inputSchema: draftSchema,
      outputSchema: z.object({ draftId: z.string(), alreadyExisted: z.boolean() }),
      annotations: annotations(false, false, true, true),
      policy: {
        sideEffects: ["Creates a draft in the configured email account"],
        cost: "No per-call paid API expected",
        recommendedPolicy: "allow",
      },
      execute: (input) =>
        Effect.gen(function* () {
          const transport = runtime.emailControls.transport;
          if (!transport?.createDraftEffect)
            return yield* new EmailComposeError({
              message: "Email draft transport is not available",
            });
          const config = getComposeEmailConfiguration();
          if (!config)
            return yield* new EmailComposeError({
              message: "SMTP credentials for michael@thiesen.dev are not available",
            });
          const fingerprint = fingerprintFor({ ...input, from: config.from });
          const reservation = yield* reserveEffect(
            "draft",
            input.idempotencyKey,
            fingerprint,
          );
          if (reservation.kind === "succeeded") {
            return { draftId: reservation.messageId, alreadyExisted: true };
          }
          const draftInput: EmailDraftInput = {
            idempotencyKey: input.idempotencyKey,
            to: input.to,
            cc: input.cc,
            bcc: input.bcc,
            subject: input.subject,
            text: input.text,
            inReplyTo: input.inReplyTo,
            references: input.references,
          };
          if (reservation.kind === "failed")
            return yield* reservationFailure("draft", reservation);
          if (reservation.kind !== "reserved" && reservation.kind !== "pending") {
            return yield* reservationFailure("draft", reservation);
          }
          const result = yield* transport
            .createDraftEffect(
              draftInput,
              reservation.kind === "pending" ? { allowAppend: false } : undefined,
            )
            .pipe(Effect.result);
          if (result._tag === "Failure") return yield* Effect.fail(result.failure);
          yield* completeEffect(
            "draft",
            input.idempotencyKey,
            fingerprint,
            true,
            result.success.draftId,
          );
          return {
            ...result.success,
            alreadyExisted:
              result.success.alreadyExisted || reservation.kind === "pending",
          };
        }),
    }),
    defineTool({
      name: "email_send",
      title: "Send Email",
      description:
        "Submit a plain-text email to SMTP and save a private Sent copy. sent means all recipients were accepted by SMTP, not final delivery. Reply callers should use the parent subject (Re: prefix), Message-ID and References. Reusing a key never retransmits.",
      inputSchema: sendSchema,
      outputSchema: z.object({
        sent: z.literal(true),
        messageId: z.string(),
        alreadySent: z.boolean(),
        sentCopy: z.enum(["pending", "uncertain", "verified", "legacy-unavailable"]),
      }),
      annotations: annotations(false, false, true, true),
      policy: {
        sideEffects: [
          "Sends an external email to the specified recipients",
          "May append a private copy in the Sent mailbox",
        ],
        cost: "Consumes SMTP provider quota",
        recommendedPolicy: "require_approval",
      },
      execute: (input) =>
        Effect.gen(function* () {
          const config = getComposeEmailConfiguration();
          if (!config)
            return yield* new EmailComposeError({
              message: "SMTP credentials for michael@thiesen.dev are not configured",
            });
          const fingerprint = fingerprintFor({ ...input, from: config.from });
          const messageId = `<${fingerprint}@omni-notify>`;
          const date = new Date(yield* Clock.currentTimeMillis);
          const prepared = yield* prepareComposedEmailEffect({
            ...input,
            messageId,
            date,
          });
          const reservation = yield* reserveEffect(
            "send",
            input.idempotencyKey,
            fingerprint,
            prepared,
          );
          if (reservation.kind === "succeeded") {
            return {
              sent: true as const,
              messageId: reservation.messageId,
              alreadySent: true,
              sentCopy: yield* saveSentEffect(runtime, input.idempotencyKey),
            };
          }
          if (reservation.kind !== "reserved")
            return yield* reservationFailure("send", reservation);
          const sent = yield* sendComposedEmailEffect({
            to: input.to,
            cc: input.cc,
            bcc: input.bcc,
            subject: input.subject,
            text: input.text,
            inReplyTo: input.inReplyTo,
            references: input.references,
            messageId,
            raw: Buffer.from(prepared.wire, "base64"),
          });
          yield* completeEffect(
            "send",
            input.idempotencyKey,
            fingerprint,
            sent,
            messageId,
          );
          if (!sent)
            return yield* new EmailComposeError({
              message:
                "Delivery was not confirmed for every recipient. Some recipients may have received the email; this operation will not be sent again automatically. Do not retry with a new key without checking delivery.",
            });
          return {
            sent: true as const,
            messageId,
            alreadySent: false,
            sentCopy: yield* saveSentEffect(runtime, input.idempotencyKey),
          };
        }),
    }),
    defineTool({
      name: "email_send_status",
      title: "Read Email Send Receipt",
      description:
        "Read the durable SMTP acceptance receipt by idempotency key. Missing or uncertain receipts do not establish delivery. This never submits SMTP or appends mail.",
      inputSchema: z.object({ idempotencyKey: composeFields.idempotencyKey }).strict(),
      outputSchema: z.object({
        found: z.boolean(),
        status: z.enum(["pending", "succeeded", "failed"]).nullable(),
        smtpAccepted: z.boolean(),
        messageId: z.string().nullable(),
        recordedAt: z.string().nullable(),
        messageDate: z.string().nullable(),
        sentCopy: z
          .enum(["pending", "uncertain", "verified", "legacy-unavailable"])
          .nullable(),
      }),
      annotations: annotations(true, false, true, false),
      policy: { sideEffects: [], cost: "No paid API", recommendedPolicy: "allow" },
      execute: (input) =>
        Effect.gen(function* () {
          const attempt = yield* readAttemptEffect(input.idempotencyKey);
          return {
            found: Boolean(attempt),
            status: attempt?.status ?? null,
            smtpAccepted:
              attempt?.status === "succeeded" && attempt.result?.sent === true,
            messageId: attempt?.result?.messageId ?? null,
            recordedAt: attempt ? new Date(attempt.updatedAt).toISOString() : null,
            messageDate: attempt?.prepared?.date ?? null,
            sentCopy: attempt ? (attempt.sentCopy ?? "legacy-unavailable") : null,
          };
        }),
    }),
    defineTool({
      name: "email_sent_copy_repair",
      title: "Repair Email Sent Copy",
      description:
        "Save or reconcile a Sent copy using a confirmed receipt and its persisted original MIME. Never retransmits SMTP. Legacy receipts without MIME cannot be reconstructed by this tool. An uncertain APPEND is only searched, never repeated.",
      inputSchema: z.object({ idempotencyKey: composeFields.idempotencyKey }).strict(),
      outputSchema: z.object({
        sentCopy: z.enum(["pending", "uncertain", "verified", "legacy-unavailable"]),
      }),
      annotations: annotations(false, false, true, true),
      policy: {
        sideEffects: ["May append one private Sent copy; never sends email"],
        cost: "No paid API",
        recommendedPolicy: "require_approval",
      },
      execute: (input) =>
        saveSentEffect(runtime, input.idempotencyKey).pipe(
          Effect.map((sentCopy) => ({ sentCopy })),
        ),
    }),
  ];
}
