import type { Logger } from "@micthiesen/mitools/logging";
import type { JmapContext } from "./client.js";

export interface FetchedEmail {
  id: string;
  subject: string;
  from: string;
  textBody: string;
  receivedAt: string;
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
      properties: ["id", "subject", "from", "textBody", "bodyValues", "receivedAt"],
      fetchTextBodyValues: true,
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
    };
    logger.debug(
      `Email: "${email.subject}" from=${email.from} ` +
        `bodyParts=${JSON.stringify(e.textBody)} ` +
        `bodyValues=${JSON.stringify(e.bodyValues)} ` +
        `textBody=${JSON.stringify(email.textBody.slice(0, 200))}`,
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

function extractTextBody(email: Record<string, unknown>): string {
  const parts = email.textBody as { partId?: string }[] | undefined;
  const bodyValues = email.bodyValues as Record<string, { value?: string }> | undefined;
  if (!parts || !bodyValues) return "";
  return parts
    .map((p) => (p.partId ? (bodyValues[p.partId]?.value ?? "") : ""))
    .join("\n");
}
