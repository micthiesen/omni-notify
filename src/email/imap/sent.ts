import { createHash } from "node:crypto";
import { Data, Effect } from "effect";
import { simpleParser, type AddressObject, type ParsedMail } from "mailparser";

export interface SentCopyInput {
  messageId: string;
  content: Buffer;
  internalDate: Date;
}

export interface SentCopyResult {
  messageId: string;
  mailbox: string;
  alreadyExisted: boolean;
}

export interface SentCopyClient {
  list(): Promise<Array<{ path: string; specialUse?: string }>>;
  getMailboxLock(
    path: string,
    options: { readOnly: boolean },
  ): Promise<{ release(): void }>;
  search(
    query: { header: Record<string, string> },
    options: { uid: true },
  ): Promise<number[] | false>;
  fetchOne(
    uid: number,
    query: { envelope: true; source: true },
    options: { uid: true },
  ): Promise<{ envelope?: { messageId?: string }; source?: Buffer } | false>;
  append(
    path: string,
    content: Buffer,
    flags: string[],
    internalDate: Date,
  ): Promise<unknown | false>;
}

export class SentCopyError extends Data.TaggedError("SentCopyError")<{
  readonly operation: string;
  readonly cause: unknown;
}> {
  public override get message(): string {
    const detail =
      this.cause instanceof Error ? this.cause.message : String(this.cause);
    return `${this.operation} failed: ${detail}`;
  }
}

const MAX_CANDIDATES = 50;

const attempt = <A>(operation: string, run: () => Promise<A>) =>
  Effect.tryPromise({
    try: run,
    catch: (cause) => new SentCopyError({ operation, cause }),
  });

const failure = (operation: string, detail: string) =>
  new SentCopyError({ operation, cause: new Error(detail) });

function addresses(value: AddressObject | AddressObject[] | undefined): string[] {
  return (value ? (Array.isArray(value) ? value : [value]) : [])
    .flatMap((group) => group.value)
    .map((address) => JSON.stringify([address.address ?? "", address.name ?? ""]));
}

/** Compare decoded MIME content so harmless wire line-ending changes are allowed. */
function semanticMessage(mail: ParsedMail): string {
  return JSON.stringify({
    messageId: mail.messageId,
    date: mail.date?.toISOString(),
    from: addresses(mail.from),
    to: addresses(mail.to),
    cc: addresses(mail.cc),
    bcc: addresses(mail.bcc),
    replyTo: addresses(mail.replyTo),
    subject: mail.subject,
    text: mail.text?.replace(/\r\n/g, "\n"),
    html: typeof mail.html === "string" ? mail.html : undefined,
    inReplyTo: mail.inReplyTo,
    references: mail.references
      ? Array.isArray(mail.references)
        ? mail.references
        : [mail.references]
      : [],
    attachments: mail.attachments.map((attachment) => ({
      name: attachment.filename,
      type: attachment.contentType,
      disposition: attachment.contentDisposition,
      cid: attachment.cid,
      digest: createHash("sha256").update(attachment.content).digest("hex"),
    })),
  });
}

function discoverSentEffect(client: SentCopyClient) {
  return Effect.gen(function* () {
    const folders = yield* attempt("LIST mailboxes", () => client.list());
    const sent = folders.filter(
      (folder) => folder.specialUse?.toLowerCase() === "\\sent",
    );
    if (sent.length !== 1) {
      return yield* failure(
        "discover Sent mailbox",
        sent.length === 0
          ? "IMAP server did not designate a Sent mailbox"
          : "IMAP server designated multiple Sent mailboxes",
      );
    }
    return sent[0].path;
  });
}

function findInSelectedSentEffect(
  client: SentCopyClient,
  mailbox: string,
  messageId: string,
  expected?: string,
) {
  return Effect.gen(function* () {
    const matches = yield* attempt("search Sent copy", () =>
      client.search({ header: { "Message-ID": messageId } }, { uid: true }),
    );
    if (matches && matches.length > MAX_CANDIDATES) {
      return yield* failure(
        "search Sent copy",
        `More than ${MAX_CANDIDATES} candidates; refusing an incomplete duplicate check`,
      );
    }
    let found: SentCopyResult | undefined;
    for (const uid of matches || []) {
      const message = yield* attempt("fetch matching Sent copy", () =>
        client.fetchOne(uid, { envelope: true, source: true }, { uid: true }),
      );
      // IMAP HEADER searches are substring matches. Compare the full identifier.
      if (!message || message.envelope?.messageId !== messageId) continue;
      if (expected !== undefined) {
        if (!message.source) {
          return yield* failure(
            "verify Sent copy",
            "Matching Message-ID has no readable MIME source",
          );
        }
        const parsed = yield* attempt("parse Sent copy", () =>
          simpleParser(message.source!),
        );
        if (semanticMessage(parsed) !== expected) {
          return yield* failure(
            "verify Sent copy",
            "Matching Message-ID belongs to different MIME content",
          );
        }
      }
      found = { messageId, mailbox, alreadyExisted: true };
    }
    return found;
  });
}

/** Read-only duplicate lookup in the uniquely designated Sent mailbox. */
export function findSentCopyEffect(client: SentCopyClient, messageId: string) {
  return Effect.gen(function* () {
    const mailbox = yield* discoverSentEffect(client);
    return yield* Effect.acquireUseRelease(
      attempt("open Sent mailbox", () =>
        client.getMailboxLock(mailbox, { readOnly: true }),
      ),
      () => findInSelectedSentEffect(client, mailbox, messageId),
      (lock) => Effect.sync(() => lock.release()),
    );
  });
}

/** Append only after a durable caller reservation; uncertain APPENDs are read-only. */
export function appendSentCopyEffect<R = never>(
  client: SentCopyClient,
  input: SentCopyInput,
  options: {
    allowAppend?: boolean;
    beforeAppend?: Effect.Effect<boolean, unknown, R>;
  } = {},
): Effect.Effect<SentCopyResult, SentCopyError, R> {
  return Effect.gen(function* () {
    if (!Number.isFinite(input.internalDate.getTime())) {
      return yield* failure("prepare Sent copy", "Invalid original internal date");
    }
    const parsed = yield* attempt("parse original Sent MIME", () =>
      simpleParser(input.content),
    );
    if (parsed.messageId !== input.messageId || !parsed.date || !parsed.from) {
      return yield* failure(
        "prepare Sent copy",
        "Original MIME must preserve the exact Message-ID, Date, and From headers",
      );
    }
    const expected = semanticMessage(parsed);
    const mailbox = yield* discoverSentEffect(client);
    return yield* Effect.acquireUseRelease(
      attempt("open Sent mailbox", () =>
        client.getMailboxLock(mailbox, { readOnly: options.allowAppend === false }),
      ),
      () =>
        Effect.gen(function* () {
          const prior = yield* findInSelectedSentEffect(
            client,
            mailbox,
            input.messageId,
            expected,
          );
          if (prior) return prior;
          if (options.allowAppend === false) {
            return yield* failure(
              "reconcile prior Sent APPEND",
              "No matching Sent copy is visible; the prior APPEND will not be repeated",
            );
          }
          if (options.beforeAppend) {
            const allowed = yield* options.beforeAppend.pipe(
              Effect.mapError(
                (cause) =>
                  new SentCopyError({ operation: "reserve Sent APPEND", cause }),
              ),
            );
            if (!allowed)
              return yield* failure(
                "reserve Sent APPEND",
                "Another attempt owns APPEND; only reconcile the existing copy",
              );
          }
          const appended = yield* attempt("APPEND Sent copy", () =>
            client.append(mailbox, input.content, ["\\Seen"], input.internalDate),
          );
          if (appended === false) {
            return yield* failure(
              "APPEND Sent copy",
              "IMAP server did not confirm APPEND; reconcile before another attempt",
            );
          }
          const verified = yield* findInSelectedSentEffect(
            client,
            mailbox,
            input.messageId,
            expected,
          );
          if (!verified) {
            return yield* failure(
              "verify Sent APPEND",
              "APPEND completed but matching MIME is not visible; reconcile before another attempt",
            );
          }
          return { ...verified, alreadyExisted: false };
        }),
      (lock) => Effect.sync(() => lock.release()),
    );
  });
}
