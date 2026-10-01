import { Effect, Schema } from "effect";
import type { Docstore } from "@micthiesen/mitools/docstore";

/** Transport-agnostic email pipeline types implemented by iCloud IMAP. */

export interface EmailAttachment {
  /** Opaque IMAP folder and UID coordinates. */
  blobId: string;
  name: string;
  type: string; // MIME type
  size: number;
}

export interface FetchedEmail {
  id: string;
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
  receivedAt: string;
  attachments: EmailAttachment[];
}

export const EmailAttachmentSchema = Schema.Struct({
  blobId: Schema.String,
  name: Schema.String,
  type: Schema.String,
  size: Schema.Number,
});

export const FetchedEmailSchema = Schema.Struct({
  id: Schema.String,
  subject: Schema.String,
  from: Schema.String,
  to: Schema.optional(Schema.Array(Schema.String)),
  cc: Schema.optional(Schema.Array(Schema.String)),
  replyTo: Schema.optional(Schema.Array(Schema.String)),
  messageId: Schema.optional(Schema.String),
  references: Schema.optional(Schema.Array(Schema.String)),
  textBody: Schema.String,
  links: Schema.Array(Schema.String),
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

export interface EmailDraftInput {
  idempotencyKey: string;
  to: string[];
  cc?: string[];
  bcc?: string[];
  subject: string;
  text: string;
  inReplyTo?: string;
  references?: string[];
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
}
