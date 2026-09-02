import { Effect, Schema } from "effect";
import type { PersistenceError } from "../effect/errors.js";

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

export interface EmailHandler<out E = unknown> {
  name: string;
  handleEmailsEffect(emails: FetchedEmail[]): Effect.Effect<void, E>;
}

export interface EmailPoll {
  emails: FetchedEmail[];
  /**
   * Persist the transport cursor covering these emails. The dispatcher calls
   * it after fan-out so a crash mid-dispatch re-delivers instead of dropping
   * (pipeline dedup gates make re-delivery safe).
   */
  commit: Effect.Effect<void, PersistenceError>;
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
  folder?: "inbox" | "archive" | "all";
  /** Maximum number of messages to return across all folders. */
  limit: number;
}

export interface EmailTransport<out E = unknown> {
  /** Short label for logs ("IMAP"). */
  readonly name: string;
  /**
   * Begin push monitoring. onMailEvent may fire spuriously (reconnects,
   * keepalives); the dispatcher polls to find out what actually changed.
   * Resolves once the initial connection is up; throws on failure so the
   * boot-retry loop can alert and try again. Later disconnects self-heal
   * with backoff inside the transport.
   */
  startEffect(onMailEvent: () => void): Effect.Effect<void, E>;
  readonly stopEffect: Effect.Effect<void, never>;
  /** Fetch emails that arrived since the persisted cursor. */
  readonly pollNewEmailsEffect: Effect.Effect<EmailPoll, E>;
  /** Re-fetch one email by its stable id (retry/reprocess); undefined when gone. */
  fetchEmailByIdEffect(id: string): Effect.Effect<FetchedEmail | undefined, E>;
  /**
   * Search the monitored mailbox without exposing transport credentials or raw
   * protocol access.
   */
  searchEmailsEffect?(options: EmailSearchOptions): Effect.Effect<FetchedEmail[], E>;
  /** Download one attachment's bytes; undefined when unavailable. */
  downloadAttachmentEffect(
    attachment: EmailAttachment,
  ): Effect.Effect<DownloadedAttachment | undefined, E>;
}
