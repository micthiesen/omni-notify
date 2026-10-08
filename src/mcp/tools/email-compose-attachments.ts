import { createHash } from "node:crypto";
import { Data, Effect, Schema } from "effect";
import { z } from "zod";
import {
  isPdfAttachment,
  MAX_ATTACHMENT_BYTES,
  safeAttachmentFilename,
  STABLE_ATTACHMENT_ID_PATTERN,
  validAttachmentMessageId,
} from "../../email/imap/attachments.js";
import type { EmailTransport, OutgoingEmailAttachment } from "../../email/types.js";

const MAX_OUTGOING_ATTACHMENTS = 5;
const MAX_OUTGOING_ATTACHMENT_TOTAL_BYTES = 10 * 1024 * 1024;
const MIB = 1024 * 1024;
const SHA256_PATTERN = /^[a-f0-9]{64}$/;

const attachmentReference = z
  .object({
    messageId: z
      .string()
      .trim()
      .max(998)
      .refine(validAttachmentMessageId, "Expected an exact RFC Message-ID")
      .describe("Exact RFC Message-ID of the email that holds the PDF"),
    attachmentId: z
      .string()
      .regex(STABLE_ATTACHMENT_ID_PATTERN, "Expected a stable attachmentId")
      .describe("Stable attachmentId from email_get or email_search"),
    sha256: z
      .string()
      .regex(SHA256_PATTERN, "Expected a lowercase hex SHA-256 digest")
      .describe("SHA-256 of the bytes you reviewed, from email_attachment_get"),
  })
  .strict();
export type OutgoingAttachmentReference = z.output<typeof attachmentReference>;

/** References only: Omni re-reads bytes server-side, so callers never upload files. */
export const outgoingAttachmentsSchema = z
  .array(attachmentReference)
  .max(MAX_OUTGOING_ATTACHMENTS)
  .refine(
    (references) =>
      new Set(references.map((ref) => `${ref.messageId}\n${ref.attachmentId}`)).size ===
      references.length,
    "Each attachment may be referenced only once",
  )
  .describe(
    `PDFs to attach. Read each one with email_attachment_get first and pass its attachmentReference ({messageId, attachmentId, sha256}) unchanged. Omni re-reads the bytes server-side and refuses any whose SHA-256 differs from what you reviewed; never pass file bytes. PDF only: up to ${MAX_OUTGOING_ATTACHMENTS} attachments, ${MAX_ATTACHMENT_BYTES / MIB} MiB each and ${MAX_OUTGOING_ATTACHMENT_TOTAL_BYTES / MIB} MiB total.`,
  )
  .optional()
  // An empty list keeps the attachment-free fingerprint and Message-ID.
  .transform((references) => (references?.length ? references : undefined));

export const OutgoingAttachmentMetadataSchema = Schema.Struct({
  messageId: Schema.String,
  attachmentId: Schema.String,
  filename: Schema.String,
  mimeType: Schema.Literal("application/pdf"),
  size: Schema.Number,
  sha256: Schema.String,
});
export type OutgoingAttachmentMetadata = typeof OutgoingAttachmentMetadataSchema.Type;

export const outgoingAttachmentMetadataOutput = z.object({
  messageId: z.string(),
  attachmentId: z.string(),
  filename: z.string(),
  mimeType: z.literal("application/pdf"),
  size: z.number().int().min(1).max(MAX_ATTACHMENT_BYTES),
  sha256: z.string(),
});

export interface ResolvedOutgoingAttachment {
  file: OutgoingEmailAttachment;
  metadata: OutgoingAttachmentMetadata;
}

export class OutgoingAttachmentError extends Data.TaggedError(
  "OutgoingAttachmentError",
)<{ readonly message: string }> {}

/**
 * A PDF filename that is never used as a path. Literal RFC 2047 markers are
 * broken up so recipients cannot decode them into header-like text.
 */
function outgoingPdfFilename(name: string | undefined): string {
  const filename = safeAttachmentFilename(name).replace(/=\?/g, "=_");
  return /\.pdf$/i.test(filename) ? filename : `${filename}.pdf`;
}

/**
 * Re-read each referenced PDF fresh before any reservation or delivery. Errors
 * name attachments by position and opaque id, never by filename or content.
 */
export function resolveOutgoingAttachmentsEffect<E, R>(
  transport: Pick<EmailTransport<E, R>, "fetchAttachmentByIdEffect"> | null | undefined,
  references: readonly OutgoingAttachmentReference[] | undefined,
): Effect.Effect<ResolvedOutgoingAttachment[], OutgoingAttachmentError, R> {
  return Effect.gen(function* () {
    if (!references?.length) return [];
    const fail = (message: string) =>
      Effect.fail(
        new OutgoingAttachmentError({
          message: `${message}. Nothing was sent or saved`,
        }),
      );
    if (!transport?.fetchAttachmentByIdEffect)
      return yield* fail("Attachment retrieval is unavailable");
    const resolved: ResolvedOutgoingAttachment[] = [];
    let totalBytes = 0;
    for (const [index, reference] of references.entries()) {
      const label = `Attachment ${index + 1} (${reference.attachmentId})`;
      const read = yield* transport
        .fetchAttachmentByIdEffect(reference.messageId, reference.attachmentId, {
          maxBytes: MAX_ATTACHMENT_BYTES,
        })
        .pipe(Effect.result);
      if (read._tag === "Failure")
        return yield* fail(
          `${label} could not be read: ${read.failure instanceof Error ? read.failure.message : String(read.failure)}`,
        );
      const attachment = read.success;
      if (!attachment)
        return yield* fail(
          `${label} was not found in Inbox, Archive or Sent; the source email may have moved or been deleted. Re-read it with email_get for a current attachmentId`,
        );
      if (attachment.data.length < 1) return yield* fail(`${label} is empty`);
      if (attachment.data.length > MAX_ATTACHMENT_BYTES)
        return yield* fail(
          `${label} exceeds the ${MAX_ATTACHMENT_BYTES / MIB} MiB attachment limit`,
        );
      if (!isPdfAttachment(attachment))
        return yield* fail(
          `${label} is not a PDF with matching MIME type and PDF header`,
        );
      const sha256 = createHash("sha256").update(attachment.data).digest("hex");
      if (sha256 !== reference.sha256)
        return yield* fail(
          `${label} no longer matches the reviewed sha256; the part changed or another email shares its Message-ID. Re-read it with email_attachment_get`,
        );
      totalBytes += attachment.data.length;
      if (totalBytes > MAX_OUTGOING_ATTACHMENT_TOTAL_BYTES)
        return yield* fail(
          `Attachments exceed the ${MAX_OUTGOING_ATTACHMENT_TOTAL_BYTES / MIB} MiB total limit`,
        );
      const filename = outgoingPdfFilename(attachment.name);
      resolved.push({
        file: { filename, contentType: "application/pdf", content: attachment.data },
        metadata: {
          messageId: reference.messageId,
          attachmentId: reference.attachmentId,
          filename,
          mimeType: "application/pdf",
          size: attachment.data.length,
          sha256,
        },
      });
    }
    return resolved;
  });
}
