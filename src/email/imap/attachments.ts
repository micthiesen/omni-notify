import { createHash } from "node:crypto";
import type { Attachment } from "mailparser";

export const MAX_ATTACHMENT_BYTES = 5 * 1024 * 1024;
export const MAX_ATTACHMENT_MESSAGE_BYTES = 20 * 1024 * 1024;

export function validAttachmentMessageId(messageId: string): boolean {
  return (
    messageId.length <= 998 &&
    /^<[^<>\s\x00-\x1f\x7f]+@[^<>\s\x00-\x1f\x7f]+>$/.test(messageId)
  );
}

/** Mailparser exposes MIME tree coordinates at runtime, but omits them in its types. */
export function attachmentPartId(attachment: Attachment): string | undefined {
  const partId: unknown = Reflect.get(attachment, "partId");
  // A single, non-multipart root attachment has no parent boundary.
  if (partId === null) return "1";
  return typeof partId === "string" && /^[1-9]\d*(?:\.[1-9]\d*)*$/.test(partId)
    ? partId
    : undefined;
}

export function encodeStableAttachmentId(messageId: string, partId: string): string {
  return `imap-attachment:${createHash("sha256")
    .update(JSON.stringify([messageId, partId]))
    .digest("hex")}`;
}

/** Preserve declared MIME identity; mailparser may otherwise infer it from a filename. */
export function declaredAttachmentMimeType(attachment: Attachment): string {
  const header = attachment.headers.get("content-type");
  const value =
    typeof header === "object" && header !== null && "value" in header
      ? header.value
      : header;
  return typeof value === "string" &&
    /^[a-z0-9!#$&^_.+-]+\/[a-z0-9!#$&^_.+-]+$/i.test(value)
    ? value.toLowerCase()
    : "application/octet-stream";
}

/** A display/download filename, never a filesystem path supplied by the sender. */
export function safeAttachmentFilename(filename: string | undefined): string {
  const name = (filename ?? "attachment")
    .replaceAll("\\", "/")
    .split("/")
    .at(-1)!
    .replace(/[\x00-\x1f\x7f\u202a-\u202e\u2066-\u2069]/g, "")
    .trim()
    .replace(/^\.+/, "")
    .slice(0, 180);
  return name || "attachment";
}
