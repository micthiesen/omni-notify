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
  /** Download one attachment's bytes; undefined when unavailable. */
  downloadAttachment(
    attachment: EmailAttachment,
  ): Promise<DownloadedAttachment | undefined>;
}
