import type { Logger } from "@micthiesen/mitools/logging";
import { ImapFlow } from "imapflow";
import { simpleParser } from "mailparser";
import { Clock, Data, Duration, Effect, Fiber, Random, Schedule } from "effect";
import { getLastDispatchedAt } from "../persistence.js";
import type {
  DownloadedAttachment,
  EmailAttachment,
  EmailPoll,
  EmailSearchOptions,
  EmailTransport,
  FetchedEmail,
} from "../types.js";
import {
  type AutoReadClient,
  discoverAutoReadFoldersEffect,
  markRecentUnreadReadEffect,
} from "./autoRead.js";
import {
  decodeAttachmentBlobId,
  type MessageCoords,
  mapParsedMessage,
} from "./mapMessage.js";
import { getFolderCursor, saveFolderCursor } from "./persistence.js";
import { planFolderSync } from "./sync.js";

const IMAP_HOST = "imap.mail.me.com";
const IMAP_PORT = 993;

/**
 * Folders whose new mail feeds the pipelines — the IMAP equivalent of the
 * JMAP inbox/archive mailbox-role scoping. Archive is watched directly
 * because iCloud server-side rules can file mail there before IMAP delivery.
 */
const FOLDERS = ["INBOX", "Archive"];

/** IDLE only pushes for the selected folder (INBOX); this sweep catches mail
 * filed straight into Archive and any pushes lost across reconnects. */
const SWEEP_INTERVAL_MS = 5 * 60_000;

/** Re-issue IDLE well before the RFC 2177 29-minute limit. */
const MAX_IDLE_TIME_MS = 13 * 60_000;

// Reconnect backoff mirrors the JMAP EventSource: first attempt jittered
// 0-3s, then doubling, capped at 5 min, reset on successful connect.
const INITIAL_BACKOFF_MS = 1_000;
const MAX_BACKOFF_MS = 5 * 60_000;

/** Safety cap on messages fetched per folder per pass (the cursor only
 * advances past what was fetched, so the rest follows next pass). */
const MAX_EMAILS_PER_PASS = 200;

/**
 * Messages older than this are cursor-skipped without dispatch. Protects the
 * pipelines (and the triage LLM bill) from bulk imports: an imapsync sweep
 * copies years of mail in with brand-new UIDs but preserved INTERNALDATEs.
 */
const MAX_EMAIL_AGE_MS = 7 * 24 * 60 * 60_000;

interface ImapAuth {
  user: string;
  pass: string;
}

export class ImapOperationError extends Data.TaggedError("ImapOperationError")<{
  readonly operation: string;
  readonly cause: unknown;
}> {
  public override get message(): string {
    const detail =
      this.cause instanceof Error ? this.cause.message : String(this.cause);
    return `${this.operation} failed: ${detail}`;
  }
}

/**
 * iCloud transport: IMAP IDLE push + per-folder UID-cursor delta fetch.
 *
 * iCloud server quirks handled here (see prompt research / MailKit #970,
 * Bugzilla #1611624): the pre-login CAPABILITY response is minimal, so
 * capabilities are only trusted post-login (imapflow re-reads them);
 * parameterized `SELECT (CONDSTORE)` is rejected, so QRESYNC stays off and
 * sync is plain-UID based; there is no MOVE, and the only mailbox mutation is
 * adding the \Seen flag for the small auto-read cleanup pass.
 */
export class ImapTransport implements EmailTransport<ImapOperationError> {
  public readonly name = "IMAP";

  private auth: ImapAuth;
  private logger: Logger;
  private client: ImapFlow | null = null;
  private onMailEvent: (() => void) | undefined;
  private sweepFiber: Fiber.RuntimeFiber<void, never> | null = null;
  private reconnectFiber: Fiber.RuntimeFiber<void, never> | null = null;
  private stopped = false;
  /** Special-use mailboxes are rediscovered after every new connection. */
  private autoReadFolders: string[] | undefined;
  /** Last bulk-import-guard skip count per folder, to de-noise repeat logs. */
  private lastSkipCounts = new Map<string, number>();
  /** imapflow mailbox selection is connection-global, so every complete
   * select/use/restore sequence and lifecycle transition shares one permit. */
  private readonly operationSemaphore = Effect.unsafeMakeSemaphore(1);
  private connectFiber: Fiber.RuntimeFiber<void, ImapOperationError> | null = null;

  constructor(auth: ImapAuth, logger: Logger) {
    this.auth = auth;
    this.logger = logger;
  }

  startEffect(onMailEvent: () => void): Effect.Effect<void, ImapOperationError> {
    return Effect.gen(this, function* () {
      this.stopped = false;
      this.onMailEvent = onMailEvent;
      // First connect fails to the boot retry boundary; later disconnects are
      // supervised by the interruptible reconnect fiber.
      yield* this.connectSingleFlightEffect;
      if (this.sweepFiber) yield* Fiber.interrupt(this.sweepFiber);
      this.sweepFiber = yield* Effect.sleep(SWEEP_INTERVAL_MS).pipe(
        Effect.tap(() => Effect.sync(() => this.onMailEvent?.())),
        Effect.forever,
        Effect.forkDaemon,
      );
      onMailEvent();
    });
  }

  readonly stopEffect: Effect.Effect<void, never> = Effect.gen(this, function* () {
    this.stopped = true;
    if (this.sweepFiber) yield* Fiber.interrupt(this.sweepFiber);
    this.sweepFiber = null;
    if (this.reconnectFiber) yield* Fiber.interrupt(this.reconnectFiber);
    this.reconnectFiber = null;
    if (this.connectFiber) yield* Fiber.interrupt(this.connectFiber);
    this.connectFiber = null;
    this.logger.info("Closing IMAP connection");
    yield* this.runSerializedEffect(
      "IMAP stop",
      Effect.gen(this, function* () {
        const client = this.client;
        this.client = null;
        if (!client) return;
        yield* Effect.tryPromise({
          try: () => client.logout(),
          catch: (cause) => new ImapOperationError({ operation: "IMAP logout", cause }),
        }).pipe(Effect.catchAll(() => Effect.sync(() => client.close())));
      }),
    ).pipe(Effect.catchAll(() => Effect.void));
  });

  private readonly connectSingleFlightEffect: Effect.Effect<void, ImapOperationError> =
    Effect.suspend(() => {
      if (this.connectFiber) return Fiber.join(this.connectFiber);
      return Effect.gen(this, function* () {
        let fiber!: Fiber.RuntimeFiber<void, ImapOperationError>;
        fiber = yield* this.runSerializedEffect(
          "IMAP connect",
          this.connectEffect,
        ).pipe(
          Effect.ensuring(
            Effect.sync(() => {
              if (this.connectFiber === fiber) this.connectFiber = null;
            }),
          ),
          // The first caller owns the connection attempt. If startup is
          // interrupted, its child is interrupted too; a non-signal-aware
          // ImapFlow connect Promise cannot outlive transport ownership.
          Effect.fork,
        );
        this.connectFiber = fiber;
        return yield* Fiber.join(fiber);
      });
    });

  private readonly connectEffect: Effect.Effect<void, ImapOperationError> =
    Effect.acquireUseRelease(
      Effect.sync(() => {
        const client = new ImapFlow({
          host: IMAP_HOST,
          port: IMAP_PORT,
          secure: true,
          auth: this.auth,
          logger: false,
          maxIdleTime: MAX_IDLE_TIME_MS,
          // qresync deliberately off: iCloud rejects `SELECT ... (CONDSTORE)`.
        });

        client.on("error", (error: Error) => {
          this.logger.warn(`IMAP connection error: ${error.message}`);
        });
        client.on("close", () => {
          const wasCurrent = this.client === client;
          if (wasCurrent) this.client = null;
          if (wasCurrent && !this.stopped) this.scheduleReconnect();
        });
        // New message in the selected mailbox (INBOX) while idling.
        client.on("exists", () => this.onMailEvent?.());
        return client;
      }),
      (client) =>
        Effect.gen(this, function* () {
          yield* this.promiseEffect("connect", () => client.connect());
          if (this.stopped) {
            return yield* new ImapOperationError({
              operation: "connect",
              cause: new Error("IMAP transport stopped while connecting"),
            });
          }

          // imapflow auto-idles on the selected mailbox whenever no command runs.
          yield* this.promiseEffect("select INBOX", () =>
            client.mailboxOpen("INBOX", { readOnly: true }),
          );

          this.autoReadFolders = undefined;
          const caps = ["IDLE", "CONDSTORE", "QRESYNC", "UIDPLUS"]
            .map((c) => `${c}=${client.capabilities.has(c) ? "y" : "n"}`)
            .join(" ");
          this.logger.info(`IMAP connected to ${IMAP_HOST} (${caps})`);
          // Ownership transfers to the transport only after connect and select
          // both succeed. The release below closes every local, untransferred
          // client on failure or interruption.
          this.client = client;
        }),
      (client) =>
        Effect.suspend(() =>
          this.client === client
            ? Effect.void
            : Effect.sync(() => {
                try {
                  client.close();
                } catch {
                  // A failed setup has no usable connection left to preserve.
                }
              }),
        ),
    );

  private scheduleReconnect(): void {
    if (this.reconnectFiber || this.stopped) return;

    const retrySchedule = Schedule.exponential(
      Duration.millis(INITIAL_BACKOFF_MS),
    ).pipe(
      Schedule.jittered,
      Schedule.modifyDelay((_, delay) =>
        Duration.min(delay, Duration.millis(MAX_BACKOFF_MS)),
      ),
    );
    let fiber: Fiber.RuntimeFiber<void, never>;
    const reconnect = Effect.gen(this, function* () {
      const initialJitter = yield* Random.nextIntBetween(0, 3_001);
      this.logger.warn(`IMAP connection closed, reconnecting in ${initialJitter}ms`);
      yield* Effect.sleep(Duration.millis(initialJitter));
      yield* this.connectSingleFlightEffect.pipe(
        Effect.tapError((error) =>
          Effect.sync(() =>
            this.logger.warn(`IMAP reconnect failed: ${error.message}`),
          ),
        ),
        Effect.retry(retrySchedule),
      );
      // Pushes during the gap are gone; treat reconnect as a mail event.
      this.onMailEvent?.();
    }).pipe(
      Effect.ensuring(
        Effect.sync(() => {
          if (this.reconnectFiber === fiber) this.reconnectFiber = null;
        }),
      ),
      Effect.catchAll(() => Effect.void),
    );
    fiber = Effect.runFork(reconnect);
    this.reconnectFiber = fiber;
  }

  readonly pollNewEmailsEffect: Effect.Effect<EmailPoll, ImapOperationError> =
    this.runSerializedEffect(
      "IMAP poll",
      Effect.gen(this, function* () {
        const client = yield* this.requireClientEffect;
        const results = yield* Effect.forEach(
          FOLDERS,
          (folder) =>
            this.pollFolderEffect(client, folder).pipe(
              Effect.catchAll((error) =>
                Effect.sync(() => {
                  this.logger.warn(
                    `IMAP poll failed for folder "${folder}": ${error.message}`,
                  );
                  return { emails: [] as FetchedEmail[], commit: undefined };
                }),
              ),
            ),
          { concurrency: 1 },
        );
        yield* this.autoReadEffect(client);
        const emails = results.flatMap((result) => result.emails);
        const commits = results.flatMap((result) =>
          result.commit ? [result.commit] : [],
        );
        return { emails, commit: () => commits.forEach((commit) => commit()) };
      }).pipe(Effect.ensuring(this.restoreInboxEffect())),
    );

  private autoReadEffect(client: ImapFlow): Effect.Effect<void, never> {
    return Effect.gen(this, function* () {
      if (this.autoReadFolders === undefined) {
        const discovered = yield* Effect.either(
          discoverAutoReadFoldersEffect(client as AutoReadClient),
        );
        if (discovered._tag === "Left") {
          this.logger.warn(
            `IMAP auto-read mailbox discovery failed: ${discovered.left.message}`,
          );
          return;
        }
        this.autoReadFolders = discovered.right;
      }
      yield* markRecentUnreadReadEffect(
        client as AutoReadClient,
        this.autoReadFolders,
        this.logger,
      );
    });
  }

  private pollFolderEffect(
    client: ImapFlow,
    folder: string,
  ): Effect.Effect<
    { emails: FetchedEmail[]; commit?: () => void },
    ImapOperationError
  > {
    return Effect.gen(this, function* () {
      const status = yield* this.promiseEffect(`STATUS ${folder}`, () =>
        client.status(folder, { uidNext: true, uidValidity: true }),
      );
      if (status.uidNext === undefined || status.uidValidity === undefined) {
        return yield* new ImapOperationError({
          operation: `STATUS ${folder}`,
          cause: new Error("STATUS returned no uidNext/uidValidity"),
        });
      }
      const uidValidity = String(status.uidValidity);
      const uidNext = status.uidNext;
      const plan = planFolderSync(getFolderCursor(folder), { uidValidity, uidNext });
      switch (plan.action) {
        case "init":
          this.logger.info(
            `First run for ${folder}: cursor initialized at uid ${uidNext} (skipping history)`,
          );
          return {
            emails: [],
            commit: () => saveFolderCursor(folder, uidValidity, uidNext),
          };
        case "none":
          return { emails: [] };
        case "reset":
          return yield* this.recoverFromUidValidityChangeEffect(
            client,
            folder,
            uidValidity,
            uidNext,
          );
        case "fetch": {
          const { emails, nextUid } = yield* this.fetchNewInFolderEffect(
            client,
            folder,
            uidValidity,
            plan.fromUid,
            uidNext,
          );
          return {
            emails,
            commit: () => saveFolderCursor(folder, uidValidity, nextUid),
          };
        }
      }
    });
  }

  private fetchNewInFolderEffect(
    client: ImapFlow,
    folder: string,
    uidValidity: string,
    fromUid: number,
    statusUidNext: number,
  ): Effect.Effect<{ emails: FetchedEmail[]; nextUid: number }, ImapOperationError> {
    return Effect.acquireUseRelease(
      this.promiseEffect(`lock ${folder}`, () =>
        client.getMailboxLock(folder, { readOnly: true }),
      ),
      () =>
        Effect.gen(this, function* () {
          const metas = yield* collectFetchMetadataEffect(client, fromUid, folder);
          const now = yield* Clock.currentTimeMillis;
          const cutoff = now - MAX_EMAIL_AGE_MS;
          const fresh = metas.filter(
            (meta) =>
              meta.internalDate === undefined || meta.internalDate.getTime() >= cutoff,
          );
          if (fresh.length < metas.length) {
            const skipped = metas.length - fresh.length;
            const level =
              this.lastSkipCounts.get(folder) === skipped ? "debug" : "info";
            this.lastSkipCounts.set(folder, skipped);
            this.logger[level](
              `${folder}: skipping ${skipped} message(s) older than ${MAX_EMAIL_AGE_MS / 86_400_000}d (bulk import guard)`,
            );
          } else {
            this.lastSkipCounts.delete(folder);
          }
          const selected = fresh.slice(0, MAX_EMAILS_PER_PASS);
          if (fresh.length > selected.length) {
            this.logger.warn(
              `${folder}: fetch pass hit the ${MAX_EMAILS_PER_PASS}-email cap; the rest follows on the next pass`,
            );
          }
          const emails = yield* Effect.forEach(
            selected,
            (meta) =>
              this.fetchMappedMessageEffect(
                client,
                folder,
                uidValidity,
                meta.uid,
                meta.internalDate,
              ),
            { concurrency: 1 },
          ).pipe(
            Effect.map((values) =>
              values.filter((email): email is FetchedEmail => email !== undefined),
            ),
          );
          const nextUid =
            fresh.length > selected.length && selected.length > 0
              ? selected[selected.length - 1].uid + 1
              : Math.max(statusUidNext, (metas.at(-1)?.uid ?? 0) + 1);
          this.logger.debug(`Fetched ${emails.length} new email(s) from ${folder}`);
          return { emails, nextUid };
        }),
      (lock) => Effect.sync(() => lock.release()),
    );
  }

  private recoverFromUidValidityChangeEffect(
    client: ImapFlow,
    folder: string,
    uidValidity: string,
    uidNext: number,
  ): Effect.Effect<
    { emails: FetchedEmail[]; commit?: () => void },
    ImapOperationError
  > {
    return Effect.gen(this, function* () {
      const lastDispatchedAt = getLastDispatchedAt();
      const commit = () => saveFolderCursor(folder, uidValidity, uidNext);
      if (lastDispatchedAt === undefined) {
        this.logger.warn(
          `${folder}: UIDVALIDITY changed with no last-dispatch timestamp; resetting cursor only`,
        );
        return { emails: [], commit };
      }
      const since = new Date(lastDispatchedAt - 60 * 60_000);
      const found = yield* Effect.acquireUseRelease(
        this.promiseEffect(`lock ${folder}`, () =>
          client.getMailboxLock(folder, { readOnly: true }),
        ),
        () =>
          this.promiseEffect(`search ${folder}`, () =>
            client.search({ since }, { uid: true }),
          ),
        (lock) => Effect.sync(() => lock.release()),
      );
      const uids = Array.isArray(found) ? found : [];
      const cursorForFetch = uids.length > 0 ? Math.min(...uids) : uidNext;
      const { emails } = yield* this.fetchNewInFolderEffect(
        client,
        folder,
        uidValidity,
        cursorForFetch,
        uidNext,
      );
      this.logger.warn(
        `${folder}: UIDVALIDITY changed; recovered ${emails.length} email(s) received since ${since.toISOString()}`,
      );
      return { emails, commit };
    });
  }

  fetchEmailByIdEffect(
    id: string,
  ): Effect.Effect<FetchedEmail | undefined, ImapOperationError> {
    return this.runSerializedEffect(
      "IMAP fetch by id",
      Effect.gen(this, function* () {
        const client = yield* this.requireClientEffect;
        const coords = decodeMessageId(id);
        for (const folder of FOLDERS) {
          if (coords && coords.folder !== folder) continue;
          const email = yield* this.findInFolderEffect(client, folder, id, coords);
          if (email) return email;
        }
        return undefined;
      }).pipe(Effect.ensuring(this.restoreInboxEffect())),
    );
  }

  searchEmailsEffect(
    options: EmailSearchOptions,
  ): Effect.Effect<FetchedEmail[], ImapOperationError> {
    return this.runSerializedEffect(
      "IMAP search",
      Effect.gen(this, function* () {
        const client = yield* this.requireClientEffect;
        const folderNames =
          options.folder === "inbox"
            ? ["INBOX"]
            : options.folder === "archive"
              ? ["Archive"]
              : FOLDERS;
        const perFolderLimit = Math.min(Math.max(1, options.limit), 50);
        const byFolder = yield* Effect.forEach(
          folderNames,
          (folder) =>
            Effect.acquireUseRelease(
              this.promiseEffect(`lock ${folder}`, () =>
                client.getMailboxLock(folder, { readOnly: true }),
              ),
              () =>
                Effect.gen(this, function* () {
                  const criteria = {
                    ...(options.query ? { text: options.query } : {}),
                    ...(options.from ? { from: options.from } : {}),
                    ...(options.to ? { to: options.to } : {}),
                    ...(options.subject ? { subject: options.subject } : {}),
                    ...(options.unread === undefined ? {} : { seen: !options.unread }),
                    ...(options.since ? { since: options.since } : {}),
                    ...(options.before ? { before: options.before } : {}),
                  };
                  const found = yield* this.promiseEffect(`search ${folder}`, () =>
                    client.search(criteria, { uid: true }),
                  );
                  if (!Array.isArray(found)) return [];

                  // IMAP SEARCH returns ascending sequence order. Read only the newest
                  // bounded slice, then merge the folders by parsed received time.
                  const uids = found.slice(-perFolderLimit).reverse();
                  const mailboxValidity = String(
                    orUndefined(client.mailbox)?.uidValidity,
                  );
                  return yield* Effect.forEach(
                    uids,
                    (uid) =>
                      this.fetchMappedMessageEffect(
                        client,
                        folder,
                        mailboxValidity,
                        uid,
                      ),
                    { concurrency: 1 },
                  ).pipe(
                    Effect.map((values) =>
                      values.filter(
                        (email): email is FetchedEmail => email !== undefined,
                      ),
                    ),
                  );
                }),
              (lock) => Effect.sync(() => lock.release()),
            ),
          { concurrency: 1 },
        );

        return byFolder
          .flat()
          .sort((a, b) => Date.parse(b.receivedAt) - Date.parse(a.receivedAt))
          .slice(0, options.limit);
      }).pipe(Effect.ensuring(this.restoreInboxEffect())),
    );
  }

  private findInFolderEffect(
    client: ImapFlow,
    folder: string,
    id: string,
    coords: (MessageCoords & { index?: number }) | undefined,
  ): Effect.Effect<FetchedEmail | undefined, ImapOperationError> {
    return Effect.acquireUseRelease(
      this.promiseEffect(`lock ${folder}`, () =>
        client.getMailboxLock(folder, { readOnly: true }),
      ),
      () =>
        Effect.gen(this, function* () {
          const mailboxValidity = String(orUndefined(client.mailbox)?.uidValidity);

          let uid: number | undefined;
          if (coords) {
            if (coords.uidValidity !== mailboxValidity) return undefined;
            uid = coords.uid;
          } else {
            const found = yield* this.promiseEffect(`find message ${id}`, () =>
              client.search({ header: { "message-id": id } }, { uid: true }),
            );
            if (Array.isArray(found) && found.length > 0) uid = found[found.length - 1];
          }
          if (uid === undefined) return undefined;

          return yield* this.fetchMappedMessageEffect(
            client,
            folder,
            mailboxValidity,
            uid,
          );
        }),
      (lock) => Effect.sync(() => lock.release()),
    );
  }

  downloadAttachmentEffect(
    attachment: EmailAttachment,
  ): Effect.Effect<DownloadedAttachment | undefined, ImapOperationError> {
    return this.runSerializedEffect(
      "IMAP attachment download",
      Effect.gen(this, function* () {
        const target = decodeAttachmentBlobId(attachment.blobId);
        if (!target) {
          this.logger.warn(`Unrecognized attachment handle: ${attachment.blobId}`);
          return undefined;
        }

        const client = yield* this.requireClientEffect;
        return yield* Effect.acquireUseRelease(
          this.promiseEffect(`lock ${target.folder}`, () =>
            client.getMailboxLock(target.folder, { readOnly: true }),
          ),
          () =>
            Effect.gen(this, function* () {
              const mailboxValidity = String(orUndefined(client.mailbox)?.uidValidity);
              if (mailboxValidity !== target.uidValidity) {
                this.logger.warn(
                  `Attachment "${attachment.name}" unavailable: ${target.folder} UIDVALIDITY changed`,
                );
                return undefined;
              }

              const full = orUndefined(
                yield* this.promiseEffect("fetch attachment message", () =>
                  client.fetchOne(String(target.uid), { source: true }, { uid: true }),
                ),
              );
              if (!full?.source) {
                this.logger.warn(
                  `Attachment "${attachment.name}" unavailable: message uid=${target.uid} is gone`,
                );
                return undefined;
              }

              const source = full.source;
              const parsed = yield* this.promiseEffect("parse attachment message", () =>
                simpleParser(source),
              );
              const part = parsed.attachments[target.index];
              if (!part) {
                this.logger.warn(
                  `Attachment "${attachment.name}" unavailable: part ${target.index} missing`,
                );
                return undefined;
              }
              return {
                name: part.filename ?? attachment.name,
                mimeType: part.contentType,
                data: part.content,
              };
            }),
          (lock) => Effect.sync(() => lock.release()),
        );
      }).pipe(Effect.ensuring(this.restoreInboxEffect())),
    );
  }

  /** Leave INBOX selected so auto-IDLE watches the right folder at rest. */
  private restoreInboxEffect(): Effect.Effect<void, never> {
    return Effect.suspend(() => {
      const client = this.client;
      if (!client?.usable || orUndefined(client.mailbox)?.path === "INBOX")
        return Effect.void;
      return this.promiseEffect("reselect INBOX", () =>
        client.mailboxOpen("INBOX", { readOnly: true }),
      ).pipe(
        Effect.catchAll((error) =>
          Effect.sync(() => {
            this.logger.debug(`Failed to reselect INBOX: ${error.message}`);
          }),
        ),
      );
    });
  }

  private readonly requireClientEffect: Effect.Effect<ImapFlow, ImapOperationError> =
    Effect.suspend(() =>
      this.client?.usable
        ? Effect.succeed(this.client)
        : Effect.fail(
            new ImapOperationError({
              operation: "access connection",
              cause: new Error("IMAP connection is not available"),
            }),
          ),
    );

  private fetchMappedMessageEffect(
    client: ImapFlow,
    folder: string,
    uidValidity: string,
    uid: number,
    fallbackDate?: Date,
  ): Effect.Effect<FetchedEmail | undefined, ImapOperationError> {
    return Effect.gen(this, function* () {
      const full = orUndefined(
        yield* this.promiseEffect(`fetch uid ${uid}`, () =>
          client.fetchOne(
            String(uid),
            { source: true, internalDate: true },
            { uid: true },
          ),
        ),
      );
      if (!full?.source) return undefined;
      const source = full.source;
      const parsed = yield* this.promiseEffect(`parse uid ${uid}`, () =>
        simpleParser(source),
      );
      const email = mapParsedMessage(
        parsed,
        { folder, uidValidity, uid },
        toDate(full.internalDate) ?? fallbackDate,
      );
      this.logger.debug(
        `Email: "${email.subject}" from=${email.from} uid=${uid} folder=${folder} attachments=${email.attachments.length}`,
      );
      return email;
    });
  }

  private promiseEffect<A>(
    operation: string,
    evaluate: () => PromiseLike<A>,
  ): Effect.Effect<A, ImapOperationError> {
    return Effect.tryPromise({
      try: () => Promise.resolve(evaluate()),
      catch: (cause) => new ImapOperationError({ operation, cause }),
    });
  }

  private runSerializedEffect<A, E>(
    operation: string,
    effect: Effect.Effect<A, E>,
  ): Effect.Effect<A, E | ImapOperationError> {
    return this.operationSemaphore
      .withPermits(1)(effect)
      .pipe(
        Effect.mapError((cause) =>
          cause instanceof ImapOperationError
            ? cause
            : new ImapOperationError({ operation, cause }),
        ),
      );
  }
}

function collectFetchMetadataEffect(
  client: ImapFlow,
  fromUid: number,
  folder: string,
): Effect.Effect<Array<{ uid: number; internalDate?: Date }>, ImapOperationError> {
  // ImapFlow exposes message ranges only as an async iterable. This adapter is
  // the single Promise boundary for consuming that library protocol.
  return Effect.tryPromise({
    try: async () => {
      const metas: Array<{ uid: number; internalDate?: Date }> = [];
      for await (const message of client.fetch(
        `${fromUid}:*`,
        { internalDate: true },
        { uid: true },
      )) {
        if (message.uid >= fromUid) {
          metas.push({ uid: message.uid, internalDate: toDate(message.internalDate) });
        }
      }
      return metas.sort((a, b) => a.uid - b.uid);
    },
    catch: (cause) => new ImapOperationError({ operation: `scan ${folder}`, cause }),
  });
}

/** imapflow types several results as `T | false`; normalize to undefined. */
function orUndefined<T>(value: T | false): T | undefined {
  return value === false ? undefined : value;
}

/** imapflow types internalDate as string | Date; normalize to Date. */
function toDate(value: string | Date | undefined): Date | undefined {
  if (value === undefined) return undefined;
  const date = value instanceof Date ? value : new Date(value);
  return Number.isNaN(date.getTime()) ? undefined : date;
}

/** Fallback message ids carry folder coordinates (see mapParsedMessage). */
function decodeMessageId(id: string): MessageCoords | undefined {
  const parts = id.split("|");
  if (parts.length !== 4 || parts[0] !== "imap") return undefined;
  const uid = Number(parts[3]);
  if (!Number.isInteger(uid)) return undefined;
  return { folder: parts[1], uidValidity: parts[2], uid };
}
