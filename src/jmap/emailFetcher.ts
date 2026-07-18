import type { Logger } from "@micthiesen/mitools/logging";
import type { JmapContext } from "./client.js";
import { extractInterestingLinks, htmlToText } from "./htmlToText.js";
import {
  getMailboxRoles,
  isEmailInAllowedMailbox,
  type MailboxRoles,
} from "./mailboxes.js";

export interface EmailAttachment {
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

export interface FetchResult {
  emails: FetchedEmail[];
  newState: string;
}

const EMAIL_PROPERTIES = [
  "id",
  "subject",
  "from",
  "textBody",
  "htmlBody",
  "bodyValues",
  "receivedAt",
  "attachments",
  "mailboxIds",
] as const;

/** Max created ids requested per Email/changes call. */
const MAX_CHANGES_PER_REQUEST = 200;
/** Safety cap on total emails fetched in one pass across hasMoreChanges pages. */
const MAX_EMAILS_PER_PASS = 500;
/** How many emails the fallback Email/query recovery may pull. */
export const RECOVERY_QUERY_LIMIT = 200;

export async function fetchNewEmails(
  ctx: JmapContext,
  sinceState: string,
  logger: Logger,
): Promise<FetchResult> {
  const roles = await getMailboxRoles(ctx, logger);

  const emails: FetchedEmail[] = [];
  let state = sinceState;
  let totalFetched = 0;

  for (;;) {
    const page = await fetchChangesPage(ctx, state, logger);
    totalFetched += page.rawEmails.length;
    for (const raw of page.rawEmails) {
      const mapped = mapAndFilterEmail(raw, roles, logger);
      if (mapped) emails.push(mapped);
    }

    const stale = page.newState === state;
    state = page.newState;
    if (!page.hasMoreChanges) break;
    if (stale) {
      logger.warn(
        "Email/changes reported more changes without advancing state; stopping pass",
      );
      break;
    }
    if (totalFetched >= MAX_EMAILS_PER_PASS) {
      logger.warn(
        `Email fetch pass hit the ${MAX_EMAILS_PER_PASS}-email cap with changes ` +
          "remaining; they will be picked up on the next pass",
      );
      break;
    }
  }

  logger.debug(`Fetched ${emails.length} new email(s)`);
  return { emails, newState: state };
}

/**
 * Fetch a single email by id (same properties/mapping as fetchNewEmails).
 * Returns undefined when the email no longer exists.
 */
export async function fetchEmailById(
  ctx: JmapContext,
  emailId: string,
  logger: Logger,
): Promise<FetchedEmail | undefined> {
  const { jam, accountId } = ctx;
  const [result] = await jam.request([
    "Email/get",
    {
      accountId,
      ids: [emailId],
      properties: EMAIL_PROPERTIES,
      fetchTextBodyValues: true,
      fetchHTMLBodyValues: true,
    },
  ]);

  const list = (result as Record<string, unknown>).list as
    | Record<string, unknown>[]
    | undefined;
  const raw = list?.[0];
  if (!raw) return undefined;
  return mapEmail(raw, logger);
}

export interface QueryFetchResult {
  emails: FetchedEmail[];
  /** The Email state observed by the query's Email/get (safe to resume from). */
  state: string;
}

/**
 * Fallback for cannotCalculateChanges gap recovery: query emails received on
 * or after `sinceMs` (newest first, bounded by RECOVERY_QUERY_LIMIT), fetch
 * and mailbox-filter them, and report the fresh Email state.
 */
export async function fetchEmailsReceivedSince(
  ctx: JmapContext,
  sinceMs: number,
  logger: Logger,
): Promise<QueryFetchResult> {
  const roles = await getMailboxRoles(ctx, logger);
  const { jam, accountId } = ctx;

  const [{ emails }] = await jam.requestMany((t) => {
    const query = t.Email.query({
      accountId,
      filter: { after: new Date(sinceMs).toISOString() },
      sort: [{ property: "receivedAt", isAscending: false }],
      limit: RECOVERY_QUERY_LIMIT,
    });

    const emails = t.Email.get({
      accountId,
      ids: query.$ref("/ids"),
      properties: EMAIL_PROPERTIES,
      fetchTextBodyValues: true,
      fetchHTMLBodyValues: true,
    });

    return { query, emails };
  });

  const response = emails as Record<string, unknown>;
  const state = response.state as string;
  const rawList = (response.list ?? []) as Record<string, unknown>[];

  const fetched: FetchedEmail[] = [];
  for (const raw of rawList) {
    const mapped = mapAndFilterEmail(raw, roles, logger);
    if (mapped) fetched.push(mapped);
  }
  return { emails: fetched, state };
}

interface ChangesPage {
  rawEmails: Record<string, unknown>[];
  newState: string;
  hasMoreChanges: boolean;
}

async function fetchChangesPage(
  ctx: JmapContext,
  sinceState: string,
  logger: Logger,
): Promise<ChangesPage> {
  const { jam, accountId } = ctx;

  const [{ changes, emails }] = await jam.requestMany((t) => {
    const changes = t.Email.changes({
      accountId,
      sinceState,
      maxChanges: MAX_CHANGES_PER_REQUEST,
    });

    const emails = t.Email.get({
      accountId,
      ids: changes.$ref("/created"),
      properties: EMAIL_PROPERTIES,
      fetchTextBodyValues: true,
      fetchHTMLBodyValues: true,
    });

    return { changes, emails };
  });

  const changesResponse = changes as Record<string, unknown>;
  const newState = changesResponse.newState as string;
  const hasMoreChanges = Boolean(changesResponse.hasMoreChanges);
  const rawEmails =
    ((emails as Record<string, unknown>).list as
      | Record<string, unknown>[]
      | undefined) ?? [];

  if (rawEmails.length === 0) {
    logger.debug("No new emails in this state change");
  }
  return { rawEmails, newState, hasMoreChanges };
}

function mapAndFilterEmail(
  raw: Record<string, unknown>,
  roles: MailboxRoles | undefined,
  logger: Logger,
): FetchedEmail | undefined {
  const mailboxIds = raw.mailboxIds as Record<string, boolean> | undefined;
  if (!isEmailInAllowedMailbox(mailboxIds, roles)) {
    logger.debug(
      `Skipping email outside inbox/archive: "${raw.subject}" from=${formatFrom(raw.from)}`,
    );
    return undefined;
  }
  return mapEmail(raw, logger);
}

function mapEmail(e: Record<string, unknown>, logger: Logger): FetchedEmail {
  const email: FetchedEmail = {
    id: e.id as string,
    subject: (e.subject as string) ?? "",
    from: formatFrom(e.from),
    textBody: extractTextBody(e),
    links: extractLinks(e),
    receivedAt: (e.receivedAt as string) ?? "",
    attachments: extractAttachments(e),
  };
  logger.debug(
    `Email: "${email.subject}" from=${email.from} ` +
      `bodyParts=${JSON.stringify(e.textBody)} ` +
      `bodyValues=${JSON.stringify(e.bodyValues)} ` +
      `textBody=${JSON.stringify(email.textBody.slice(0, 200))} ` +
      `attachments=${email.attachments.length}`,
  );
  return email;
}

function formatFrom(from: unknown): string {
  if (!Array.isArray(from) || from.length === 0) return "";
  const first = from[0] as { email?: string; name?: string };
  return first.email ?? first.name ?? "";
}

interface BodyPart {
  partId: string;
  type?: string;
}

function isBodyPartArray(value: unknown): value is BodyPart[] {
  return (
    Array.isArray(value) &&
    value.every(
      (p) => typeof p === "object" && p !== null && typeof p.partId === "string",
    )
  );
}

function isBodyValues(value: unknown): value is Record<string, { value?: string }> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function extractTextBody(email: Record<string, unknown>): string {
  if (!isBodyValues(email.bodyValues)) return "";
  const bodyValues = email.bodyValues;

  // Prefer HTML body: it's typically more complete than plain text (some senders
  // render fields like appointment times only in HTML, leaving "undefined" in text).
  if (isBodyPartArray(email.htmlBody)) {
    const html = extractParts(email.htmlBody, bodyValues, true);
    if (html) return html;
  }

  if (isBodyPartArray(email.textBody)) {
    return extractParts(email.textBody, bodyValues, false);
  }

  return "";
}

function extractParts(
  parts: BodyPart[],
  bodyValues: Record<string, { value?: string }>,
  convertHtml: boolean,
): string {
  return parts
    .map((p) => {
      const value = bodyValues[p.partId]?.value ?? "";
      if (!value) return "";
      if (convertHtml) return htmlToText(value);
      return p.type !== "text/plain" ? htmlToText(value) : value;
    })
    .join("\n");
}

function extractLinks(email: Record<string, unknown>): string[] {
  if (!isBodyValues(email.bodyValues)) return [];
  if (!isBodyPartArray(email.htmlBody)) return [];
  const bodyValues = email.bodyValues;
  const html = email.htmlBody.map((p) => bodyValues[p.partId]?.value ?? "").join("\n");
  return extractInterestingLinks(html);
}

function extractAttachments(email: Record<string, unknown>): EmailAttachment[] {
  const attachments = email.attachments as
    | { blobId?: string; name?: string; type?: string; size?: number }[]
    | undefined;
  if (!attachments) return [];
  return attachments
    .filter(
      (a): a is typeof a & { blobId: string; type: string } => !!a.blobId && !!a.type,
    )
    .map((a) => ({
      blobId: a.blobId,
      name: a.name ?? "unnamed",
      type: a.type,
      size: a.size ?? 0,
    }));
}
