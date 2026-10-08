import { Effect, Schema } from "effect";
import type { Docstore } from "@micthiesen/mitools/docstore";
import { EmailLinkMetadataSchema, type EmailLinkMetadata } from "./linkMetadata.js";

/** Transport-agnostic email pipeline types implemented by iCloud IMAP. */

export interface EmailAttachment {
  /** Opaque IMAP folder and UID coordinates. */
  blobId: string;
  /** Stable exact Message-ID plus MIME part handle, independent of folder/UID. */
  attachmentId?: string;
  partId?: string;
  disposition?: string;
  contentId?: string;
  name: string;
  type: string; // MIME type
  size: number;
}

export interface FetchedEmail {
  id: string;
  /** Exact mailbox identity at the time of this read. A later move changes it. */
  origin?: { folder: string; uidValidity: string; uid: number };
  subject: string;
  from: string;
  to?: string[];
  cc?: string[];
  replyTo?: string[];
  messageId?: string;
  references?: string[];
  inReplyTo?: string;
  textBody: string;
  /** Shipment/booking-shaped URLs pulled from the HTML body (hrefs are
   * stripped from textBody, but tracking numbers often live only in them). */
  links: string[];
  /** Untrusted private link targets. Never log or include in routine reports. */
  linkMetadata?: EmailLinkMetadata;
  receivedAt: string;
  attachments: EmailAttachment[];
}

export const EmailAttachmentSchema = Schema.Struct({
  blobId: Schema.String,
  attachmentId: Schema.optional(Schema.String),
  partId: Schema.optional(Schema.String),
  disposition: Schema.optional(Schema.String),
  contentId: Schema.optional(Schema.String),
  name: Schema.String,
  type: Schema.String,
  size: Schema.Number,
});

export const FetchedEmailSchema = Schema.Struct({
  id: Schema.String,
  origin: Schema.optional(
    Schema.Struct({
      folder: Schema.String,
      uidValidity: Schema.String,
      uid: Schema.Number,
    }),
  ),
  subject: Schema.String,
  from: Schema.String,
  to: Schema.optional(Schema.Array(Schema.String)),
  cc: Schema.optional(Schema.Array(Schema.String)),
  replyTo: Schema.optional(Schema.Array(Schema.String)),
  messageId: Schema.optional(Schema.String),
  references: Schema.optional(Schema.Array(Schema.String)),
  textBody: Schema.String,
  links: Schema.Array(Schema.String),
  linkMetadata: Schema.optional(EmailLinkMetadataSchema),
  receivedAt: Schema.String,
  attachments: Schema.Array(EmailAttachmentSchema),
});

export interface DownloadedAttachment {
  name: string;
  mimeType: string;
  data: Buffer;
}

export interface EmailHandler<out E = unknown, out R = never> {
  name: string;
  handleEmailsEffect(emails: FetchedEmail[]): Effect.Effect<void, E, R>;
}

export interface EmailPoll {
  emails: FetchedEmail[];
  /**
   * Persist the transport cursor covering these emails. The dispatcher calls
   * it after fan-out so a crash mid-dispatch re-delivers instead of dropping
   * (pipeline dedup gates make re-delivery safe).
   */
  commit: Effect.Effect<void, unknown, Docstore>;
}

export interface EmailSearchOptions {
  /** Full-text query across headers and body. */
  query?: string;
  /** Sender address or name fragment. */
  from?: string;
  /** Recipient address or name fragment. */
  to?: string;
  /** Subject fragment. */
  subject?: string;
  /** When set, restrict results by read/unread state. */
  unread?: boolean;
  /** IMAP internal date lower bound (inclusive, day precision). */
  since?: Date;
  /** IMAP internal date upper bound (exclusive, day precision). */
  before?: Date;
  /** Which monitored folder(s) to search. */
  folder?: "inbox" | "archive" | "sent" | "all";
  /** Maximum number of messages to return across all folders. */
  limit: number;
  /** Bypass recent search and parsed-message caches. */
  fresh?: boolean;
}

/** Verified bytes re-read from the mailbox by stable identity, never caller-supplied. */
export interface OutgoingEmailAttachment {
  filename: string;
  contentType: "application/pdf";
  content: Buffer;
}

export interface EmailDraftInput {
  idempotencyKey: string;
  to: string[];
  cc?: string[];
  bcc?: string[];
  subject: string;
  text: string;
  inReplyTo?: string;
  references?: string[];
  /** Every entry must be appended; an implementation that cannot must fail. */
  attachments?: readonly OutgoingEmailAttachment[];
}

export interface EmailDraftResult {
  /** Stable Message-ID used to reconcile an uncertain IMAP APPEND. */
  draftId: string;
  alreadyExisted: boolean;
}

export interface EmailTransport<out E = unknown, out R = never> {
  /** Short label for logs ("IMAP"). */
  readonly name: string;
  /**
   * Begin push monitoring. onMailEvent may fire spuriously (reconnects,
   * keepalives); the dispatcher polls to find out what actually changed.
   * Resolves once the initial connection is up; throws on failure so the
   * boot-retry loop can alert and try again. Later disconnects self-heal
   * with backoff inside the transport.
   */
  startEffect(onMailEvent: () => void): Effect.Effect<void, E, R>;
  readonly stopEffect: Effect.Effect<void, never, R>;
  /** Fetch emails that arrived since the persisted cursor. */
  readonly pollNewEmailsEffect: Effect.Effect<EmailPoll, E, R>;
  /** Re-fetch one email by its stable id (retry/reprocess); undefined when gone. */
  fetchEmailByIdEffect(
    id: string,
    options?: { fresh?: boolean },
  ): Effect.Effect<FetchedEmail | undefined, E, R>;
  /**
   * Search the monitored mailbox without exposing transport credentials or raw
   * protocol access.
   */
  searchEmailsEffect?(options: EmailSearchOptions): Effect.Effect<FetchedEmail[], E, R>;
  /** Read exact source MIME and flags before durably claiming a native MOVE. */
  inspectArchiveMessageEffect?(
    identity: import("./imap/archive.js").ArchiveIdentity,
  ): Effect.Effect<import("./imap/archive.js").ArchiveSnapshot, E, R>;
  /** One native IMAP MOVE of an exact Inbox message to the designated Archive. */
  moveArchiveMessageEffect?(
    identity: import("./imap/archive.js").ArchiveIdentity,
    sourceHash: string,
  ): Effect.Effect<import("./imap/archive.js").ArchiveMoveResult, E, R>;
  /** Read-only check used after uncertain MOVE outcomes and restarts. */
  reconcileArchiveMessageEffect?(
    identity: import("./imap/archive.js").ArchiveIdentity,
    sourceHash?: string,
  ): Effect.Effect<import("./imap/archive.js").ArchiveReconcileResult, E, R>;
  /** Reverse one recorded archive move by exact Archive coordinates. */
  restoreArchiveMessageEffect?(
    identity: import("./imap/archive.js").ArchiveIdentity,
    destination: import("./imap/archive.js").ArchiveLocation,
    sourceHash: string,
  ): Effect.Effect<import("./imap/archive.js").ArchiveMoveResult, E, R>;
  inspectArchiveDestinationEffect?(
    identity: import("./imap/archive.js").ArchiveIdentity,
    destination: import("./imap/archive.js").ArchiveLocation,
  ): Effect.Effect<import("./imap/archive.js").ArchiveSnapshot, E, R>;
  verifyArchiveLocationEffect?(
    location: import("./imap/archive.js").ArchiveLocation,
    messageId: string,
    sourceHash: string,
    flags: readonly string[],
  ): Effect.Effect<boolean, E, R>;
  reconcileRestoreMessageEffect?(
    identity: import("./imap/archive.js").ArchiveIdentity,
    destination: import("./imap/archive.js").ArchiveLocation,
    sourceHash: string,
  ): Effect.Effect<import("./imap/archive.js").ArchiveReconcileResult, E, R>;
  copyExactArchiveMessageEffect?(
    source: import("./imap/archive.js").ArchiveIdentity,
    targetFolder: string,
    snapshot: import("./imap/archive.js").ArchiveSnapshot,
  ): Effect.Effect<import("./imap/archive.js").ArchiveLocation, E, R>;
  reconcileExactCopyEffect?(
    source: import("./imap/archive.js").ArchiveIdentity,
    targetFolder: string,
    snapshot: import("./imap/archive.js").ArchiveSnapshot,
  ): Effect.Effect<import("./imap/archive.js").ArchiveReconcileResult, E, R>;
  markExactArchiveSourceDeletedEffect?(
    source: import("./imap/archive.js").ArchiveIdentity,
    destination: import("./imap/archive.js").ArchiveLocation,
    snapshot: import("./imap/archive.js").ArchiveSnapshot,
  ): Effect.Effect<boolean, E, R>;
  inspectExactDeletedSourceEffect?(
    source: import("./imap/archive.js").ArchiveIdentity,
    destination: import("./imap/archive.js").ArchiveLocation,
    snapshot: import("./imap/archive.js").ArchiveSnapshot,
  ): Effect.Effect<"marked" | "unmarked" | "absent" | "uncertain", E, R>;
  expungeExactArchiveSourceEffect?(
    source: import("./imap/archive.js").ArchiveIdentity,
    destination: import("./imap/archive.js").ArchiveLocation,
    snapshot: import("./imap/archive.js").ArchiveSnapshot,
  ): Effect.Effect<boolean, E, R>;
  /** Save or reconcile a private Sent copy; never submits SMTP. */
  saveSentCopyEffect?(
    input: import("./imap/sent.js").SentCopyInput,
    options?: {
      allowAppend?: boolean;
      beforeAppend?: Effect.Effect<boolean, unknown, R>;
    },
  ): Effect.Effect<import("./imap/sent.js").SentCopyResult, E, R>;
  /** Save a draft in the server-designated Drafts mailbox. */
  createDraftEffect?(
    input: EmailDraftInput,
    options?: { allowAppend?: boolean },
  ): Effect.Effect<EmailDraftResult, E, R>;
  /** Download one attachment's bytes; undefined when unavailable. */
  downloadAttachmentEffect(
    attachment: EmailAttachment,
  ): Effect.Effect<DownloadedAttachment | undefined, E, R>;
  /** Bounded private read by stable Message-ID and MIME identity; never marks read. */
  fetchAttachmentByIdEffect?(
    messageId: string,
    attachmentId: string,
    options?: { maxBytes?: number },
  ): Effect.Effect<DownloadedAttachment | undefined, E, R>;
}
