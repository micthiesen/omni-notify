import { createHash } from "node:crypto";
import { decodeDoc, Docstore } from "@micthiesen/mitools/docstore";
import { causeMessage } from "@micthiesen/mitools/errors";
import { Clock, Data, Effect, Option, Schema } from "effect";
import { z } from "zod";
import type { EmailDraftInput } from "../../email/types.js";
import { getComposeEmailConfiguration } from "../../emails/client.js";
import { sendComposedEmailEffect } from "../../emails/send.js";
import { prepareComposedEmailEffect } from "../../emails/mime.js";
import type { McpRuntime } from "../runtime.js";
import { annotations, defineTool, type McpToolDefinition } from "../tool.js";
import {
  OutgoingAttachmentMetadataSchema,
  outgoingAttachmentMetadataOutput,
  outgoingAttachmentsSchema,
  resolveOutgoingAttachmentsEffect,
  type OutgoingAttachmentMetadata,
} from "./email-compose-attachments.js";

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
  attachments: outgoingAttachmentsSchema,
};

export class EmailComposeError extends Data.TaggedError("EmailComposeError")<{
  readonly message: string;
  readonly cause?: unknown;
}> {}

/** MCP error text reports only the innermost cause, so keep guidance in the message. */
const failWith = (message: string) => (cause: unknown) =>
  new EmailComposeError({ message: `${message}: ${causeMessage(cause)}` });

/** Receipts drop MIME no later step uses: wire after SMTP, the copy once verified. */
const PreparedMessageSchema = Schema.Struct({
  from: Schema.String,
  date: Schema.String,
  wire: Schema.optional(Schema.String),
  content: Schema.optional(Schema.String),
});
type PreparedMessage = typeof PreparedMessageSchema.Type;

function retainedMime(prepared: PreparedMessage | undefined, keepContent: boolean) {
  if (!prepared) return {};
  const { from, date, content } = prepared;
  return {
    prepared: { from, date, ...(keepContent && content ? { content } : {}) },
  };
}
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
  attachments: Schema.optional(Schema.Array(OutgoingAttachmentMetadataSchema)),
  updatedAt: Schema.Number,
});
type StoredAttempt = typeof StoredAttemptSchema.Type;

type StoredAttachments = readonly OutgoingAttachmentMetadata[];
type Reservation =
  | { kind: "reserved" }
  | { kind: "succeeded"; messageId: string; attachments: StoredAttachments }
  | { kind: "pending"; attachments: StoredAttachments }
  | { kind: "failed" }
  | { kind: "mismatch" };

const keyFor = (kind: "draft" | "send", key: string) =>
  `email-compose:${kind}:${createHash("sha256").update(key).digest("hex")}`;
const fingerprintFor = (input: unknown) =>
  createHash("sha256").update(JSON.stringify(input)).digest("hex");

function existingReservation(
  prior: StoredAttempt,
  fingerprint: string,
): Exclude<Reservation, { kind: "reserved" }> {
  if (prior.fingerprint !== fingerprint) return { kind: "mismatch" };
  const attachments = prior.attachments ?? [];
  if (prior.status === "succeeded" && prior.result?.sent) {
    return { kind: "succeeded", messageId: prior.result.messageId, attachments };
  }
  if (prior.status === "pending") return { kind: "pending", attachments };
  return { kind: "failed" };
}

function reserveEffect(
  kind: "draft" | "send",
  idempotencyKey: string,
  fingerprint: string,
  extra: { prepared?: PreparedMessage; attachments?: StoredAttachments } = {},
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
      if (prior) return existingReservation(prior, fingerprint);
      const { prepared, attachments } = extra;
      const row: StoredAttempt = {
        fingerprint,
        status: "pending",
        updatedAt: now,
        ...(prepared ? { prepared, sentCopy: "pending" as const } : {}),
        ...(attachments?.length ? { attachments } : {}),
      };
      tx.upsertDoc(pk, row, { entity: `email-compose-${kind}` }, now);
      return { kind: "reserved" };
    });
  }).pipe(Effect.mapError(failWith(`Could not reserve email ${kind}`)));
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
      if (
        prior.fingerprint === fingerprint &&
        prior.status === "succeeded" &&
        prior.result?.sent === sent &&
        prior.result.messageId === messageId
      ) {
        // A concurrent reconciler recorded the same outcome first.
        return;
      }
      if (prior.fingerprint !== fingerprint || prior.status !== "pending") {
        throw new Error("Email compose reservation changed unexpectedly");
      }
      tx.upsertDoc(
        pk,
        {
          ...prior,
          ...retainedMime(prior.prepared, true),
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
      failWith(`Could not persist email ${kind} outcome; do not repeat with a new key`),
    ),
  );
}

function readAttemptEffect(kind: "draft" | "send", idempotencyKey: string) {
  return Effect.gen(function* () {
    const docstore = yield* Docstore;
    const row = yield* docstore.getRawRow(keyFor(kind, idempotencyKey));
    if (Option.isNone(row)) return undefined;
    return yield* Schema.decodeUnknownEffect(StoredAttemptSchema)(
      decodeDoc<unknown>(row.value.data),
    );
  }).pipe(Effect.mapError(failWith(`Could not read ${kind} receipt`)));
}

/** Settle known keys before re-reading attachment sources or composing MIME. */
function findReservationEffect(
  kind: "draft" | "send",
  idempotencyKey: string,
  fingerprint: string,
) {
  return readAttemptEffect(kind, idempotencyKey).pipe(
    Effect.map((prior) =>
      prior ? existingReservation(prior, fingerprint) : undefined,
    ),
  );
}

/** Claim APPEND durably; a crash or lost response permits read-only reconciliation only. */
function saveSentEffect(runtime: McpRuntime, idempotencyKey: string) {
  return Effect.gen(function* () {
    const attempt = yield* readAttemptEffect("send", idempotencyKey);
    if (!attempt?.result?.sent || attempt.status !== "succeeded")
      return yield* new EmailComposeError({
        message:
          "SMTP acceptance is not confirmed; Sent copy repair cannot send or assume delivery",
      });
    if (attempt.sentCopy === "verified") return "verified" as const;
    const content = attempt.prepared?.content;
    if (!attempt.prepared || !content) return "legacy-unavailable" as const;
    const transport = runtime.emailControls.transport;
    const saveSentCopy = transport?.saveSentCopyEffect?.bind(transport);
    if (!saveSentCopy) return attempt.sentCopy ?? "pending";
    const { messageId } = attempt.result;
    const { date } = attempt.prepared;
    return yield* Effect.gen(function* () {
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
      const copied = yield* saveSentCopy(
        {
          messageId,
          content: Buffer.from(content, "base64"),
          internalDate: new Date(date),
        },
        { allowAppend: attempt.sentCopy === "pending", beforeAppend },
      ).pipe(Effect.result);
      if (copied._tag === "Failure")
        return (
          (yield* readAttemptEffect("send", idempotencyKey))?.sentCopy ?? "pending"
        );
      yield* docstore.transaction("verify Sent copy receipt", (tx) => {
        const row = tx.getRawRow(keyFor("send", idempotencyKey), now);
        if (!row) throw new Error("Send receipt disappeared");
        const prior = Schema.decodeUnknownSync(StoredAttemptSchema)(
          decodeDoc<unknown>(row.data),
        );
        tx.upsertDoc(
          keyFor("send", idempotencyKey),
          { ...prior, ...retainedMime(prior.prepared, false), sentCopy: "verified" },
          { entity: "email-compose-send" },
          now,
        );
      });
      return "verified" as const;
    }).pipe(
      Effect.mapError(
        failWith("SMTP was accepted but Sent copy persistence failed; do not resend"),
      ),
    );
  });
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
        "Save a plain-text draft in the server-designated Drafts mailbox, optionally attaching PDFs you reviewed with email_attachment_get by passing each attachmentReference unchanged. Omni re-reads attachments server-side and refuses bytes that differ from the reviewed SHA-256; never pass file bytes. A draft lets the owner review attachments before sending. Reusing an idempotency key with different content is rejected.",
      inputSchema: draftSchema,
      outputSchema: z.object({
        draftId: z.string(),
        alreadyExisted: z.boolean(),
        attachments: z.array(outgoingAttachmentMetadataOutput),
      }),
      annotations: annotations(false, false, true, true),
      policy: {
        sideEffects: [
          "Creates a draft in the configured email account, including copies of any referenced private PDF attachments",
        ],
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
          const existing = yield* findReservationEffect(
            "draft",
            input.idempotencyKey,
            fingerprint,
          );
          const attachments = existing
            ? []
            : yield* resolveOutgoingAttachmentsEffect(transport, input.attachments);
          const reservation =
            existing ??
            (yield* reserveEffect("draft", input.idempotencyKey, fingerprint, {
              attachments: attachments.map(({ metadata }) => metadata),
            }));
          if (reservation.kind === "succeeded") {
            return {
              draftId: reservation.messageId,
              alreadyExisted: true,
              attachments: reservation.attachments,
            };
          }
          if (reservation.kind !== "reserved" && reservation.kind !== "pending") {
            return yield* reservationFailure("draft", reservation);
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
            attachments: attachments.map(({ file }) => file),
          };
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
            attachments:
              reservation.kind === "pending"
                ? reservation.attachments
                : attachments.map(({ metadata }) => metadata),
          };
        }),
    }),
    defineTool({
      name: "email_send",
      title: "Send Email",
      description:
        "Submit a plain-text email to SMTP and save a private Sent copy. Optional attachments are PDFs you reviewed with email_attachment_get: pass each attachmentReference ({messageId, attachmentId, sha256}) unchanged. Omni re-reads them fresh server-side (never pass file bytes) and fails before sending if any is missing, not a PDF, over the size limits, or different from the reviewed SHA-256. When requesting approval, name each attachment's filename and source email. sent means all recipients were accepted by SMTP, not final delivery. Reply callers should use the parent subject (Re: prefix), Message-ID and References. Reusing a key never retransmits.",
      inputSchema: sendSchema,
      outputSchema: z.object({
        sent: z.literal(true),
        messageId: z.string(),
        alreadySent: z.boolean(),
        sentCopy: z.enum(["pending", "uncertain", "verified", "legacy-unavailable"]),
        attachments: z.array(outgoingAttachmentMetadataOutput),
      }),
      annotations: annotations(false, false, true, true),
      policy: {
        sideEffects: [
          "Sends an external email, including any referenced private PDF attachments, to the specified recipients",
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
          const alreadySent = (
            reservation: Extract<Reservation, { kind: "succeeded" }>,
          ) =>
            saveSentEffect(runtime, input.idempotencyKey).pipe(
              Effect.map((sentCopy) => ({
                sent: true as const,
                messageId: reservation.messageId,
                alreadySent: true,
                sentCopy,
                attachments: reservation.attachments,
              })),
            );
          const existing = yield* findReservationEffect(
            "send",
            input.idempotencyKey,
            fingerprint,
          );
          if (existing?.kind === "succeeded") return yield* alreadySent(existing);
          if (existing) return yield* reservationFailure("send", existing);
          const attachments = yield* resolveOutgoingAttachmentsEffect(
            runtime.emailControls.transport,
            input.attachments,
          );
          const metadata = attachments.map((attachment) => attachment.metadata);
          const messageId = `<${fingerprint}@omni-notify>`;
          const date = new Date(yield* Clock.currentTimeMillis);
          const prepared = yield* prepareComposedEmailEffect({
            to: input.to,
            cc: input.cc,
            bcc: input.bcc,
            subject: input.subject,
            text: input.text,
            inReplyTo: input.inReplyTo,
            references: input.references,
            attachments: attachments.map(({ file }) => file),
            messageId,
            date,
          });
          const reservation = yield* reserveEffect(
            "send",
            input.idempotencyKey,
            fingerprint,
            { prepared, attachments: metadata },
          );
          if (reservation.kind === "succeeded") return yield* alreadySent(reservation);
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
            attachments: metadata,
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
        attachments: z.array(outgoingAttachmentMetadataOutput),
      }),
      annotations: annotations(true, false, true, false),
      policy: { sideEffects: [], cost: "No paid API", recommendedPolicy: "allow" },
      execute: (input) =>
        Effect.gen(function* () {
          const attempt = yield* readAttemptEffect("send", input.idempotencyKey);
          return {
            found: Boolean(attempt),
            status: attempt?.status ?? null,
            smtpAccepted:
              attempt?.status === "succeeded" && attempt.result?.sent === true,
            messageId: attempt?.result?.messageId ?? null,
            recordedAt: attempt ? new Date(attempt.updatedAt).toISOString() : null,
            messageDate: attempt?.prepared?.date ?? null,
            sentCopy: attempt ? (attempt.sentCopy ?? "legacy-unavailable") : null,
            attachments: attempt?.attachments ?? [],
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
