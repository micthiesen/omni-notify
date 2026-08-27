/**
 * Transport-agnostic email pipeline types. Two transports implement
 * EmailTransport: Fastmail JMAP (src/email/jmap/) and iCloud IMAP
 * (src/email/imap/), selected via EMAIL_TRANSPORT during the Fastmail →
 * iCloud migration.
 */

export interface EmailAttachment {
  /** Opaque transport-specific handle (JMAP blobId / IMAP folder+uid coords). */
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

export interface DownloadedAttachment {
  name: string;
  mimeType: string;
  data: Buffer;
}

export interface EmailHandler {
  name: string;
  handleEmails(emails: FetchedEmail[]): Promise<void>;
}

export interface EmailPoll {
  emails: FetchedEmail[];
  /**
   * Persist the transport cursor covering these emails. The dispatcher calls
   * it after fan-out so a crash mid-dispatch re-delivers instead of dropping
   * (pipeline dedup gates make re-delivery safe).
   */
  commit: () => void;
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

export interface EmailTransport {
  /** Short label for logs ("JMAP", "IMAP"). */
  readonly name: string;
  /**
   * Begin push monitoring. onMailEvent may fire spuriously (reconnects,
   * keepalives); the dispatcher polls to find out what actually changed.
   * Resolves once the initial connection is up; throws on failure so the
   * boot-retry loop can alert and try again. Later disconnects self-heal
   * with backoff inside the transport.
   */
  start(onMailEvent: () => void): Promise<void>;
  stop(): void;
  /** Fetch emails that arrived since the persisted cursor. */
  pollNewEmails(): Promise<EmailPoll>;
  /** Re-fetch one email by its stable id (retry/reprocess); undefined when gone. */
  fetchEmailById(id: string): Promise<FetchedEmail | undefined>;
  /**
   * Search the monitored mailbox without exposing transport credentials or raw
   * protocol access. Optional because the retiring JMAP adapter does not offer
   * this capability; the active iCloud IMAP adapter does.
   */
  searchEmails?(options: EmailSearchOptions): Promise<FetchedEmail[]>;
  /** Download one attachment's bytes; undefined when unavailable. */
  downloadAttachment(
    attachment: EmailAttachment,
  ): Promise<DownloadedAttachment | undefined>;
}
