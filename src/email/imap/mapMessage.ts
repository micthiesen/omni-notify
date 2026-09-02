import type { ParsedMail } from "mailparser";
import { extractInterestingLinks, htmlToText } from "../htmlToText.js";
import type { EmailAttachment, FetchedEmail } from "../types.js";

/** Where a message physically lives right now (UIDs are per-folder). */
export interface MessageCoords {
  folder: string;
  uidValidity: string;
  uid: number;
}

const BLOB_PREFIX = "imap";

/** Encode folder coordinates into an opaque attachment handle. */
export function encodeAttachmentBlobId(coords: MessageCoords, index: number): string {
  return [BLOB_PREFIX, coords.folder, coords.uidValidity, coords.uid, index].join("|");
}

export function decodeAttachmentBlobId(
  blobId: string,
): (MessageCoords & { index: number }) | undefined {
  const parts = blobId.split("|");
  if (parts.length !== 5 || parts[0] !== BLOB_PREFIX) return undefined;
  const uid = Number(parts[3]);
  const index = Number(parts[4]);
  if (!Number.isInteger(uid) || !Number.isInteger(index)) return undefined;
  return { folder: parts[1], uidValidity: parts[2], uid, index };
}

/**
 * Map a parsed RFC 822 message to the pipeline shape. The id is the RFC
 * Message-ID when present — stable across folder moves (INBOX → Archive
 * changes the UID), which retry/reprocess re-fetches rely on — with the
 * folder coordinates as a fallback.
 */
export function mapParsedMessage(
  parsed: ParsedMail,
  coords: MessageCoords,
  internalDate: Date | undefined,
): FetchedEmail {
  const html = typeof parsed.html === "string" ? parsed.html : undefined;

  // Prefer HTML body: it's typically more complete than plain text (some
  // senders render fields like appointment times only in HTML).
  const textBody = html ? htmlToText(html) : (parsed.text ?? "");

  const from = parsed.from?.value?.[0];

  const attachments: EmailAttachment[] = parsed.attachments.map((a, index) => ({
    blobId: encodeAttachmentBlobId(coords, index),
    name: a.filename ?? "unnamed",
    type: a.contentType,
    size: a.size,
  }));

  return {
    id:
      parsed.messageId ??
      [BLOB_PREFIX, coords.folder, coords.uidValidity, coords.uid].join("|"),
    subject: parsed.subject ?? "",
    from: from?.address ?? from?.name ?? "",
    textBody,
    links: html ? extractInterestingLinks(html) : [],
    // INTERNALDATE is the server's receive time;
    // the Date header is sender-controlled and only a fallback.
    receivedAt: (internalDate ?? parsed.date)?.toISOString() ?? "",
    attachments,
  };
}
