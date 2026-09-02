import type { Logger } from "@micthiesen/mitools/logging";
import { Data, Effect, Schema } from "effect";
import { extractInterestingLinks, htmlToText } from "../htmlToText.js";
import type { EmailAttachment, FetchedEmail } from "../types.js";
import type { JmapContext } from "./client.js";
import {
  getMailboxRolesEffect,
  isEmailInAllowedMailbox,
  type MailboxRoles,
} from "./mailboxes.js";

export interface FetchResult {
  emails: FetchedEmail[];
  newState: string;
}

export class JmapFetchError extends Data.TaggedError("JmapFetchError")<{
  readonly operation: string;
  readonly cause: unknown;
}> {
  public override get message(): string {
    const detail =
      this.cause instanceof Error ? this.cause.message : String(this.cause);
    return `${this.operation} failed: ${detail}`;
  }
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
/** Page size for the fallback Email/query recovery drain. */
export const RECOVERY_QUERY_LIMIT = 200;

const EmailAddressSchema = Schema.Struct({
  email: Schema.optional(Schema.NullOr(Schema.String)),
  name: Schema.optional(Schema.NullOr(Schema.String)),
});

const EmailBodyPartSchema = Schema.Struct({
  partId: Schema.String,
  type: Schema.optional(Schema.String),
});

const EmailBodyValueSchema = Schema.Struct({
  value: Schema.optional(Schema.String),
});

const EmailAttachmentSchema = Schema.Struct({
  blobId: Schema.optional(Schema.String),
  name: Schema.optional(Schema.NullOr(Schema.String)),
  type: Schema.optional(Schema.String),
  size: Schema.optional(Schema.Number),
});

const JmapEmailSchema = Schema.Struct({
  id: Schema.String,
  subject: Schema.optional(Schema.NullOr(Schema.String)),
  from: Schema.optional(Schema.NullOr(Schema.Array(EmailAddressSchema))),
  textBody: Schema.optional(Schema.NullOr(Schema.Array(EmailBodyPartSchema))),
  htmlBody: Schema.optional(Schema.NullOr(Schema.Array(EmailBodyPartSchema))),
  bodyValues: Schema.optional(Schema.Record(Schema.String, EmailBodyValueSchema)),
  receivedAt: Schema.optional(Schema.NullOr(Schema.String)),
  attachments: Schema.optional(Schema.NullOr(Schema.Array(EmailAttachmentSchema))),
  mailboxIds: Schema.optional(Schema.Record(Schema.String, Schema.Boolean)),
});

type JmapEmail = typeof JmapEmailSchema.Type;
type EmailBodyValues = NonNullable<JmapEmail["bodyValues"]>;

const EmailGetSchema = Schema.Struct({
  state: Schema.String,
  list: Schema.Array(JmapEmailSchema),
});
const EmailChangesSchema = Schema.Struct({
  newState: Schema.String,
  hasMoreChanges: Schema.Boolean,
});
const EmailQuerySchema = Schema.Struct({
  ids: Schema.Array(Schema.String),
});

export const fetchNewEmailsEffect = Effect.fn("JmapEmail.fetchNew")(function* (
  ctx: JmapContext,
  sinceState: string,
  logger: Logger,
) {
  const roles = yield* getMailboxRolesEffect(ctx, logger);

  const emails: FetchedEmail[] = [];
  let state = sinceState;
  let totalFetched = 0;

  for (;;) {
    const page = yield* fetchChangesPageEffect(ctx, state, logger);
    totalFetched += page.rawEmails.length;
    for (const raw of page.rawEmails) {
      const mapped = yield* Effect.try({
        try: () => mapAndFilterEmail(raw, roles, logger),
        catch: (cause) => new JmapFetchError({ operation: "decode email", cause }),
      });
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
});

/**
 * Fetch a single email by id (same properties/mapping as fetchNewEmails).
 * Returns undefined when the email no longer exists.
 */
export const fetchEmailByIdEffect = Effect.fn("JmapEmail.fetchById")(function* (
  ctx: JmapContext,
  emailId: string,
  logger: Logger,
) {
  const { jam, accountId } = ctx;
  const [result] = yield* Effect.tryPromise({
    try: () =>
      jam.request([
        "Email/get",
        {
          accountId,
          ids: [emailId],
          properties: EMAIL_PROPERTIES,
          fetchTextBodyValues: true,
          fetchHTMLBodyValues: true,
        },
      ]),
    catch: (cause) => new JmapFetchError({ operation: "Email/get", cause }),
  });

  const response = yield* decodeEmailGetEffect(result, "decode Email/get");
  const raw = response.list[0];
  if (!raw) return undefined;
  return yield* Effect.try({
    try: () => mapEmail(raw, logger),
    catch: (cause) => new JmapFetchError({ operation: "decode email", cause }),
  });
});

export interface QueryFetchResult {
  emails: FetchedEmail[];
  /** The Email state observed by the query's Email/get (safe to resume from). */
  state: string;
}

/**
 * Fallback for cannotCalculateChanges gap recovery: query emails received on
 * or after `sinceMs`, drain every page, mailbox-filter the results, and report
 * the Email state captured before the drain. Mail arriving during recovery is
 * therefore picked up by the next normal Email/changes pass.
 */
export const fetchEmailsReceivedSinceEffect = Effect.fn("JmapEmail.fetchReceivedSince")(
  function* (ctx: JmapContext, sinceMs: number, logger: Logger) {
    const roles = yield* getMailboxRolesEffect(ctx, logger);
    const { jam, accountId } = ctx;

    const [stateResult] = yield* Effect.tryPromise({
      try: () => jam.request(["Email/get", { accountId, ids: [], properties: ["id"] }]),
      catch: (cause) => new JmapFetchError({ operation: "Email/get state", cause }),
    });
    const recoveryState = yield* Effect.try({
      try: () => Schema.decodeUnknownSync(EmailGetSchema)(stateResult).state,
      catch: (cause) =>
        new JmapFetchError({ operation: "decode recovery state", cause }),
    });

    const fetched: FetchedEmail[] = [];
    let position = 0;
    for (;;) {
      const [{ query, emails }] = yield* Effect.tryPromise({
        try: () =>
          jam.requestMany((t) => {
            const query = t.Email.query({
              accountId,
              filter: { after: new Date(sinceMs).toISOString() },
              sort: [{ property: "receivedAt", isAscending: true }],
              position,
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
          }),
        catch: (cause) => new JmapFetchError({ operation: "Email/query", cause }),
      });

      const ids = yield* Effect.try({
        try: () => Schema.decodeUnknownSync(EmailQuerySchema)(query).ids,
        catch: (cause) => new JmapFetchError({ operation: "decode query ids", cause }),
      });
      const { list: rawList } = yield* decodeEmailGetEffect(emails, "decode query");

      for (const raw of rawList) {
        const mapped = yield* Effect.try({
          try: () => mapAndFilterEmail(raw, roles, logger),
          catch: (cause) => new JmapFetchError({ operation: "decode email", cause }),
        });
        if (mapped) fetched.push(mapped);
      }
      if (ids.length < RECOVERY_QUERY_LIMIT) break;
      position += ids.length;
    }
    return { emails: fetched, state: recoveryState };
  },
);

interface ChangesPage {
  rawEmails: readonly JmapEmail[];
  newState: string;
  hasMoreChanges: boolean;
}

function fetchChangesPageEffect(
  ctx: JmapContext,
  sinceState: string,
  logger: Logger,
): Effect.Effect<ChangesPage, JmapFetchError> {
  return Effect.gen(function* () {
    const { jam, accountId } = ctx;

    const [{ changes, emails }] = yield* Effect.tryPromise({
      try: () =>
        jam.requestMany((t) => {
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
        }),
      catch: (cause) => new JmapFetchError({ operation: "Email/changes", cause }),
    });

    const { newState, hasMoreChanges, rawEmails } = yield* Effect.try({
      try: () => {
        const decodedChanges = Schema.decodeUnknownSync(EmailChangesSchema)(changes);
        const decodedEmails = Schema.decodeUnknownSync(EmailGetSchema)(emails);
        return {
          ...decodedChanges,
          rawEmails: decodedEmails.list,
        };
      },
      catch: (cause) => new JmapFetchError({ operation: "decode changes", cause }),
    });

    if (rawEmails.length === 0) {
      logger.debug("No new emails in this state change");
    }
    return { rawEmails, newState, hasMoreChanges };
  });
}

function mapAndFilterEmail(
  raw: JmapEmail,
  roles: MailboxRoles,
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

function mapEmail(e: JmapEmail, logger: Logger): FetchedEmail {
  const email: FetchedEmail = {
    id: e.id,
    subject: e.subject ?? "",
    from: formatFrom(e.from),
    textBody: extractTextBody(e),
    links: extractLinks(e),
    receivedAt: e.receivedAt ?? "",
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

function formatFrom(from: JmapEmail["from"]): string {
  if (!from || from.length === 0) return "";
  const first = from[0];
  return first.email ?? first.name ?? "";
}

function extractTextBody(email: JmapEmail): string {
  if (!email.bodyValues) return "";
  const bodyValues = email.bodyValues;

  // Prefer HTML body: it's typically more complete than plain text (some senders
  // render fields like appointment times only in HTML, leaving "undefined" in text).
  if (email.htmlBody) {
    const html = extractParts(email.htmlBody, bodyValues, true);
    if (html) return html;
  }

  if (email.textBody) {
    return extractParts(email.textBody, bodyValues, false);
  }

  return "";
}

function extractParts(
  parts: readonly (typeof EmailBodyPartSchema.Type)[],
  bodyValues: EmailBodyValues,
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

function extractLinks(email: JmapEmail): string[] {
  if (!email.bodyValues || !email.htmlBody) return [];
  const bodyValues = email.bodyValues;
  const html = email.htmlBody.map((p) => bodyValues[p.partId]?.value ?? "").join("\n");
  return extractInterestingLinks(html);
}

function extractAttachments(email: JmapEmail): EmailAttachment[] {
  const attachments = email.attachments;
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

function decodeEmailGetEffect(
  response: unknown,
  operation: string,
): Effect.Effect<typeof EmailGetSchema.Type, JmapFetchError> {
  return Effect.try({
    try: () => Schema.decodeUnknownSync(EmailGetSchema)(response),
    catch: (cause) => new JmapFetchError({ operation, cause }),
  });
}
