import type { Logger } from "@micthiesen/mitools/logging";
import type { JmapContext } from "./client.js";
import { htmlToText } from "./htmlToText.js";

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
  receivedAt: string;
  attachments: EmailAttachment[];
}

export interface FetchResult {
  emails: FetchedEmail[];
  newState: string;
}

export async function fetchNewEmails(
  ctx: JmapContext,
  sinceState: string,
  logger: Logger,
): Promise<FetchResult> {
  const { jam, accountId } = ctx;

  const [{ changes, emails }] = await jam.requestMany((t) => {
    const changes = t.Email.changes({
      accountId,
      sinceState,
    });

    const emails = t.Email.get({
      accountId,
      ids: changes.$ref("/created"),
      properties: [
        "id",
        "subject",
        "from",
        "textBody",
        "htmlBody",
        "bodyValues",
        "receivedAt",
        "attachments",
      ],
      fetchTextBodyValues: true,
      fetchHTMLBodyValues: true,
    });

    return { changes, emails };
  });

  const newState = (changes as Record<string, unknown>).newState as string;
  const emailList = (emails as Record<string, unknown>).list as
    | Record<string, unknown>[]
    | undefined;

  if (!emailList) {
    logger.debug("No new emails in this state change");
    return { emails: [], newState };
  }

  const fetched: FetchedEmail[] = emailList.map((e) => {
    const email: FetchedEmail = {
      id: e.id as string,
      subject: (e.subject as string) ?? "",
      from: formatFrom(e.from),
      textBody: extractTextBody(e),
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
  });

  logger.debug(`Fetched ${fetched.length} new email(s)`);
  return { emails: fetched, newState };
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
