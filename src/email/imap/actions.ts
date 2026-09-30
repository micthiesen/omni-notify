import { createHash } from "node:crypto";
import MailComposer from "nodemailer/lib/mail-composer/index.js";
import { Data, Effect } from "effect";
import type { EmailDraftInput, EmailDraftResult } from "../types.js";

export interface ImapDraftClient {
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
    query: { envelope: true },
    options: { uid: true },
  ): Promise<{ envelope?: { messageId?: string } } | false>;
  append(path: string, content: Buffer, flags: string[]): Promise<unknown | false>;
}

export class ImapDraftError extends Data.TaggedError("ImapDraftError")<{
  readonly operation: string;
  readonly cause: unknown;
}> {
  public override get message(): string {
    const detail =
      this.cause instanceof Error ? this.cause.message : String(this.cause);
    return `${this.operation} failed: ${detail}`;
  }
}

export function deterministicDraftMessageId(idempotencyKey: string): string {
  const digest = createHash("sha256").update(idempotencyKey).digest("hex");
  return `<${digest}@omni-notify>`;
}

/** Resolve an uncertain prior append before creating a draft with the same key. */
export function createDraftEffect(
  client: ImapDraftClient,
  input: EmailDraftInput,
  from: string,
  options: { allowAppend?: boolean } = {},
): Effect.Effect<EmailDraftResult, ImapDraftError> {
  const messageId = deterministicDraftMessageId(input.idempotencyKey);
  return Effect.gen(function* () {
    const drafts = yield* Effect.tryPromise({
      try: () => client.list(),
      catch: (cause) => new ImapDraftError({ operation: "LIST mailboxes", cause }),
    });
    const mailbox = drafts.find(
      (folder) => folder.specialUse?.toLowerCase() === "\\drafts",
    );
    if (!mailbox) {
      return yield* new ImapDraftError({
        operation: "discover Drafts mailbox",
        cause: new Error("IMAP server did not designate a Drafts mailbox"),
      });
    }

    return yield* Effect.acquireUseRelease(
      Effect.tryPromise({
        try: () => client.getMailboxLock(mailbox.path, { readOnly: false }),
        catch: (cause) =>
          new ImapDraftError({ operation: "open Drafts mailbox", cause }),
      }),
      () =>
        Effect.gen(function* () {
          const matches = yield* Effect.tryPromise({
            try: () =>
              client.search({ header: { "Message-ID": messageId } }, { uid: true }),
            catch: (cause) => new ImapDraftError({ operation: "search draft", cause }),
          });
          if (matches && matches.length > 0) {
            for (const uid of matches) {
              const found = yield* Effect.tryPromise({
                try: () => client.fetchOne(uid, { envelope: true }, { uid: true }),
                catch: (cause) =>
                  new ImapDraftError({ operation: "fetch matching draft", cause }),
              });
              if (
                found &&
                found.envelope?.messageId?.toLowerCase() === messageId.toLowerCase()
              ) {
                return { draftId: messageId, alreadyExisted: true };
              }
            }
          }

          if (options.allowAppend === false) {
            return yield* new ImapDraftError({
              operation: "reconcile prior draft APPEND",
              cause: new Error(
                "No matching draft is visible; the prior APPEND will not be repeated",
              ),
            });
          }

          const mime = new MailComposer({
            from,
            to: input.to,
            cc: input.cc,
            bcc: input.bcc,
            subject: input.subject,
            text: input.text,
            messageId,
            inReplyTo: input.inReplyTo,
            references: input.references,
          });
          const message = mime.compile();
          message.keepBcc = true;
          const content = yield* Effect.tryPromise({
            try: () => message.build(),
            catch: (cause) => new ImapDraftError({ operation: "compose draft", cause }),
          });
          const appendResult = yield* Effect.tryPromise({
            try: () => client.append(mailbox.path, content, ["\\Draft"]),
            catch: (cause) => new ImapDraftError({ operation: "APPEND draft", cause }),
          });
          if (appendResult === false) {
            return yield* new ImapDraftError({
              operation: "APPEND draft",
              cause: new Error("IMAP server did not confirm the APPEND"),
            });
          }
          const appendedMatches = yield* Effect.tryPromise({
            try: () =>
              client.search({ header: { "Message-ID": messageId } }, { uid: true }),
            catch: (cause) =>
              new ImapDraftError({ operation: "verify appended draft", cause }),
          });
          let verified = false;
          for (const uid of appendedMatches || []) {
            const found = yield* Effect.tryPromise({
              try: () => client.fetchOne(uid, { envelope: true }, { uid: true }),
              catch: (cause) =>
                new ImapDraftError({ operation: "verify appended draft", cause }),
            });
            if (
              found &&
              found.envelope?.messageId?.toLowerCase() === messageId.toLowerCase()
            ) {
              verified = true;
              break;
            }
          }
          if (!verified) {
            return yield* new ImapDraftError({
              operation: "verify appended draft",
              cause: new Error(
                "APPEND completed but the deterministic Message-ID is not yet visible",
              ),
            });
          }
          return { draftId: messageId, alreadyExisted: false };
        }),
      (heldLock) => Effect.sync(() => heldLock.release()),
    );
  });
}
