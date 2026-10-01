import {
  Logger,
  type LoggerShape,
  type NamedLogger,
} from "@micthiesen/mitools/logging";
import { ImapFlow } from "imapflow";
import { simpleParser } from "mailparser";
import {
  Clock,
  Data,
  Duration,
  Effect,
  Exit,
  Fiber,
  Random,
  Schedule,
  Scope,
  Semaphore,
  Stream,
} from "effect";
import { getLastDispatchedAtEffect } from "../persistence.js";
import type {
  EmailAttachment,
  EmailDraftInput,
  EmailSearchOptions,
  EmailTransport,
  FetchedEmail,
} from "../types.js";
import {
  type AutoReadClient,
  discoverAutoReadMailboxPlanEffect,
  markRecentUnreadReadEffect,
} from "./autoRead.js";
import { archiveAutoReadProtectionEffect } from "../archive/persistence.js";
import {
  decodeAttachmentBlobId,
  type MessageCoords,
  mapParsedMessage,
} from "./mapMessage.js";
import { getFolderCursorEffect, saveFolderCursorEffect } from "./persistence.js";
import { planFolderSync } from "./sync.js";
import { BoundedReadCache } from "./readCache.js";
import { createDraftEffect as appendDraftEffect } from "./actions.js";
import { getComposeEmailConfiguration } from "../../emails/client.js";

import { appendSentCopyEffect, type SentCopyInput } from "./sent.js";
import {
  inspectArchiveSourceEffect,
  moveArchiveMessageEffect as nativeMoveArchiveMessageEffect,
  reconcileArchiveMessageEffect as readArchiveMoveEffect,
  restoreArchiveMessageEffect as nativeRestoreArchiveMessageEffect,
  inspectArchiveDestinationEffect as inspectNativeArchiveDestinationEffect,
  verifyArchiveLocationEffect as verifyNativeArchiveLocationEffect,
  reconcileRestoreMessageEffect as readRestoreMoveEffect,
  copyExactArchiveMessageEffect as copyExactMessageEffect,
  reconcileExactCopyEffect as readExactCopyEffect,
  markExactArchiveSourceDeletedEffect as markExactDeletedEffect,
  inspectExactDeletedSourceEffect as readExactDeletedEffect,
  expungeExactArchiveSourceEffect as expungeExactSourceEffect,
  type ArchiveIdentity,
  type ArchiveLocation,
  type ArchiveSnapshot,
} from "./archive.js";
import { uidExpungeExact } from "./uidExpunge.js";
import {
  attachmentPartId,
  declaredAttachmentMimeType,
  encodeStableAttachmentId,
  MAX_ATTACHMENT_BYTES,
  MAX_ATTACHMENT_MESSAGE_BYTES,
  safeAttachmentFilename,
  validAttachmentMessageId,
} from "./attachments.js";

const IMAP_HOST = "imap.mail.me.com";
const IMAP_PORT = 993;

/**
 * Folders whose new mail feeds the pipelines — the IMAP equivalent of the
 * Inbox/archive mailbox scoping. Archive is watched directly
 * because iCloud server-side rules can file mail there before IMAP delivery.
 */
const FOLDERS = ["INBOX", "Archive"];

/** IDLE only pushes for the selected folder (INBOX); this sweep catches mail
 * filed straight into Archive and any pushes lost across reconnects. */
const SWEEP_INTERVAL_MS = 5 * 60_000;

/** Re-issue IDLE well before the RFC 2177 29-minute limit. */
const MAX_IDLE_TIME_MS = 13 * 60_000;

// Reconnect backoff starts with a jittered first attempt
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
const PARSED_CACHE_TTL_MS = 5 * 60_000;
const SEARCH_CACHE_TTL_MS = 30_000;

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
 * sync is plain-UID based; drafts are appended to the discovered special-use
 * mailbox, and the auto-read cleanup adds the \Seen flag.
 */
export class ImapTransport implements EmailTransport<
  ImapOperationError,
  import("@micthiesen/mitools/docstore").Docstore | Logger
> {
  public readonly name = "IMAP";

  private auth: ImapAuth;
  private logger: NamedLogger;
  private loggerService?: LoggerShape;
  private client: ImapFlow | null = null;
  private onMailEvent: (() => void) | undefined;
  private runtimeScope: Scope.Closeable | null = null;
  private reconnectScheduled = false;
  private stopped = false;
  /** Special-use mailboxes are rediscovered after every new connection. */
  private autoReadFolders: string[] | undefined;
  private autoReadArchiveFolders: string[] | undefined;
  /** Last bulk-import-guard skip count per folder, to de-noise repeat logs. */
  private lastSkipCounts = new Map<string, number>();
  /** imapflow mailbox selection is connection-global, so every complete
   * select/use/restore sequence and lifecycle transition shares one permit. */
  private readonly operationSemaphore = Semaphore.makeUnsafe(1);
  private connectFiber: Fiber.Fiber<void, ImapOperationError> | null = null;
  private readonly parsedMessageCache = new BoundedReadCache<FetchedEmail>(
    128,
    24 * 1024 * 1024,
    PARSED_CACHE_TTL_MS,
  );
  private readonly searchResultCache = new BoundedReadCache<FetchedEmail[]>(
    32,
    8 * 1024 * 1024,
    SEARCH_CACHE_TTL_MS,
  );
  private readonly messageLocationCache = new BoundedReadCache<MessageCoords>(
    256,
    128 * 1024,
    SEARCH_CACHE_TTL_MS,
  );
  private readonly directGetCache = new BoundedReadCache<FetchedEmail>(
    128,
    24 * 1024 * 1024,
    SEARCH_CACHE_TTL_MS,
  );

  private clearReadCaches(): void {
    this.parsedMessageCache.clear();
    this.searchResultCache.clear();
    this.messageLocationCache.clear();
    this.directGetCache.clear();
  }

  constructor(auth: ImapAuth, logger: NamedLogger) {
    this.auth = auth;
    this.logger = logger;
  }

  startEffect(onMailEvent: () => void) {
    return Effect.uninterruptibleMask((restore) =>
      Effect.gen({ self: this }, function* () {
        this.loggerService = yield* Logger;
        const previousScope = this.runtimeScope;
        this.runtimeScope = null;
        if (previousScope) {
          yield* Scope.close(previousScope, Exit.succeed(undefined));
        }
        const scope = yield* Scope.make();
        this.runtimeScope = scope;
        this.stopped = false;
        this.onMailEvent = onMailEvent;
        // First connect fails to the boot retry boundary; later disconnects are
        // supervised by children of the retained runtime Scope.
        yield* restore(this.connectSingleFlightEffect).pipe(
          Effect.onExit((exit) =>
            Exit.isSuccess(exit)
              ? Effect.void
              : Scope.close(scope, exit).pipe(
                  Effect.tap(() =>
                    Effect.sync(() => {
                      if (this.runtimeScope === scope) this.runtimeScope = null;
                    }),
                  ),
                ),
          ),
        );
        yield* Effect.sleep(SWEEP_INTERVAL_MS).pipe(
          Effect.tap(() => Effect.sync(() => this.onMailEvent?.())),
          Effect.forever,
          Effect.forkScoped,
          Effect.provideService(Scope.Scope, scope),
        );
        onMailEvent();
      }),
    );
  }

  readonly stopEffect = Effect.gen({ self: this }, function* () {
    this.stopped = true;
    this.clearReadCaches();
    const scope = this.runtimeScope;
    this.runtimeScope = null;
    if (scope) yield* Scope.close(scope, Exit.succeed(undefined));
    this.reconnectScheduled = false;
    if (this.connectFiber) yield* Fiber.interrupt(this.connectFiber);
    this.connectFiber = null;
    yield* this.logger.info("Closing IMAP connection");
    yield* this.runSerializedEffect(
      "IMAP stop",
      Effect.gen({ self: this }, function* () {
        const client = this.client;
        this.client = null;
        if (!client) return;
        yield* Effect.tryPromise({
          try: () => client.logout(),
          catch: (cause) => new ImapOperationError({ operation: "IMAP logout", cause }),
        }).pipe(Effect.catch(() => Effect.sync(() => client.close())));
      }),
    ).pipe(Effect.catch(() => Effect.void));
  });

  private readonly connectSingleFlightEffect = Effect.suspend(() => {
    if (this.connectFiber) return Fiber.join(this.connectFiber);
    return Effect.gen({ self: this }, function* () {
      let fiber!: Fiber.Fiber<void, ImapOperationError>;
      fiber = yield* this.runSerializedEffect("IMAP connect", this.connectEffect).pipe(
        Effect.ensuring(
          Effect.sync(() => {
            if (this.connectFiber === fiber) this.connectFiber = null;
          }),
        ),
        // The first caller owns the connection attempt. If startup is
        // interrupted, its child is interrupted too; a non-signal-aware
        // ImapFlow connect Promise cannot outlive transport ownership.
        Effect.forkChild,
      );
      this.connectFiber = fiber;
      return yield* Fiber.join(fiber);
    });
  });

  private readonly connectEffect = Effect.acquireUseRelease(
    Effect.gen({ self: this }, function* () {
      const loggerService = yield* Logger;
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
        Effect.runFork(
          this.logger
            .warn(`IMAP connection error: ${error.message}`)
            .pipe(Effect.provideService(Logger, loggerService)),
        );
      });
      client.on("close", () => {
        const wasCurrent = this.client === client;
        if (wasCurrent) {
          this.client = null;
          this.clearReadCaches();
        }
        if (wasCurrent && !this.stopped) this.scheduleReconnect();
      });
      // New message in the selected mailbox (INBOX) while idling.
      client.on("exists", () => {
        this.searchResultCache.clear();
        this.onMailEvent?.();
      });
      client.on("flags", () => this.searchResultCache.clear());
      client.on("expunge", () => this.clearReadCaches());
      return client;
    }),
    (client) =>
      Effect.gen({ self: this }, function* () {
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
        this.autoReadArchiveFolders = undefined;
        this.clearReadCaches();
        const caps = ["IDLE", "CONDSTORE", "QRESYNC", "UIDPLUS", "MOVE"]
          .map((c) => `${c}=${client.capabilities.has(c) ? "y" : "n"}`)
          .join(" ");
        yield* this.logger.info(`IMAP connected to ${IMAP_HOST} (${caps})`);
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
    const scope = this.runtimeScope;
    if (this.reconnectScheduled || this.stopped || !scope) return;
    this.reconnectScheduled = true;

    const retrySchedule = Schedule.exponential(
      Duration.millis(INITIAL_BACKOFF_MS),
    ).pipe(
      Schedule.jittered,
      Schedule.modifyDelay(({ duration }) =>
        Effect.succeed(Duration.min(duration, Duration.millis(MAX_BACKOFF_MS))),
      ),
    );
    const reconnect = Effect.gen({ self: this }, function* () {
      const initialJitter = yield* Random.nextIntBetween(0, 3_001);
      yield* this.logger.warn(
        `IMAP connection closed, reconnecting in ${initialJitter}ms`,
      );
      yield* Effect.sleep(Duration.millis(initialJitter));
      yield* this.connectSingleFlightEffect.pipe(
        Effect.tapError((error) =>
          this.logger.warn(`IMAP reconnect failed: ${error.message}`),
        ),
        Effect.retry(retrySchedule),
      );
      // Pushes during the gap are gone; treat reconnect as a mail event.
      this.onMailEvent?.();
    }).pipe(
      Effect.ensuring(
        Effect.sync(() => {
          this.reconnectScheduled = false;
        }),
      ),
      Effect.catch(() => Effect.void),
    );
    Effect.runFork(
      reconnect.pipe(
        Effect.forkScoped,
        Effect.provideService(Scope.Scope, scope),
        Effect.provideService(Logger, this.loggerService!),
        Effect.asVoid,
      ),
    );
  }

  readonly pollNewEmailsEffect = this.runSerializedEffect(
    "IMAP poll",
    Effect.gen({ self: this }, function* () {
      const client = yield* this.requireClientEffect;
      const results = yield* Effect.forEach(
        FOLDERS,
        (folder) =>
          this.pollFolderEffect(client, folder).pipe(
            Effect.catch((error) =>
              this.logger
                .warn(`IMAP poll failed for folder "${folder}": ${error.message}`)
                .pipe(Effect.as({ emails: [] as FetchedEmail[], commit: undefined })),
            ),
          ),
        { concurrency: 1 },
      );
      yield* this.autoReadEffect(client);
      const emails = results.flatMap((result) => result.emails);
      const commits = results.flatMap((result) =>
        result.commit ? [result.commit] : [],
      );
      return {
        emails,
        commit: Effect.all(commits, { concurrency: 1, discard: true }),
      };
    }).pipe(Effect.ensuring(this.restoreInboxEffect())),
  );

  private autoReadEffect(client: ImapFlow) {
    return Effect.gen({ self: this }, function* () {
      if (this.autoReadFolders === undefined) {
        const discovered = yield* Effect.result(
          discoverAutoReadMailboxPlanEffect(client as AutoReadClient),
        );
        if (discovered._tag === "Failure") {
          yield* this.logger.warn(
            `IMAP auto-read mailbox discovery failed: ${discovered.failure.message}`,
          );
          return;
        }
        this.autoReadFolders = discovered.success.folders;
        this.autoReadArchiveFolders = discovered.success.archiveFolders;
      }
      yield* markRecentUnreadReadEffect(
        client as AutoReadClient,
        this.autoReadFolders,
        this.logger,
        (folder, validity) =>
          this.autoReadArchiveFolders?.includes(folder)
            ? archiveAutoReadProtectionEffect(folder, validity)
            : Effect.succeed({ skip: false, excludedUids: new Set<number>() }),
      );
    });
  }

  private pollFolderEffect(client: ImapFlow, folder: string) {
    return Effect.gen({ self: this }, function* () {
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
      const cursor = yield* getFolderCursorEffect(folder).pipe(
        Effect.mapError(
          (cause) =>
            new ImapOperationError({ operation: `read cursor ${folder}`, cause }),
        ),
      );
      const plan = planFolderSync(cursor, { uidValidity, uidNext });
      switch (plan.action) {
        case "init":
          yield* this.logger.info(
            `First run for ${folder}: cursor initialized at uid ${uidNext} (skipping history)`,
          );
          return {
            emails: [],
            commit: saveFolderCursorEffect(folder, uidValidity, uidNext),
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
            commit: saveFolderCursorEffect(folder, uidValidity, nextUid),
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
  ) {
    return Effect.acquireUseRelease(
      this.promiseEffect(`lock ${folder}`, () =>
        client.getMailboxLock(folder, { readOnly: true }),
      ),
      () =>
        Effect.gen({ self: this }, function* () {
          const { metas, complete } = yield* collectFetchMetadataEffect(
            client,
            fromUid,
            folder,
          );
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
            yield* this.logger[level](
              `${folder}: skipping ${skipped} message(s) older than ${MAX_EMAIL_AGE_MS / 86_400_000}d (bulk import guard)`,
            );
          } else {
            this.lastSkipCounts.delete(folder);
          }
          const selected = fresh.slice(0, MAX_EMAILS_PER_PASS);
          if (fresh.length > selected.length) {
            yield* this.logger.warn(
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
            !complete || fresh.length > selected.length
              ? (selected.at(-1) ?? metas.at(-1))!.uid + 1
              : Math.max(statusUidNext, (metas.at(-1)?.uid ?? 0) + 1);
          yield* this.logger.debug(
            `Fetched ${emails.length} new email(s) from ${folder}`,
          );
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
  ) {
    return Effect.gen({ self: this }, function* () {
      const lastDispatchedAt = yield* getLastDispatchedAtEffect.pipe(
        Effect.mapError(
          (cause) =>
            new ImapOperationError({
              operation: "read dispatch watermark",
              cause,
            }),
        ),
      );
      if (lastDispatchedAt === undefined) {
        yield* this.logger.warn(
          `${folder}: UIDVALIDITY changed with no last-dispatch timestamp; resetting cursor only`,
        );
        return {
          emails: [],
          commit: saveFolderCursorEffect(folder, uidValidity, uidNext),
        };
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
      const { emails, nextUid } = yield* this.fetchNewInFolderEffect(
        client,
        folder,
        uidValidity,
        cursorForFetch,
        uidNext,
      );
      yield* this.logger.warn(
        `${folder}: UIDVALIDITY changed; recovered ${emails.length} email(s) received since ${since.toISOString()}`,
      );
      return {
        emails,
        commit: saveFolderCursorEffect(folder, uidValidity, nextUid),
      };
    });
  }

  fetchEmailByIdEffect(id: string, options: { fresh?: boolean } = {}) {
    return Effect.gen({ self: this }, function* () {
      const startedAt = yield* Clock.currentTimeMillis;
      const cached = options.fresh ? undefined : this.directGetCache.get(id, startedAt);
      const email =
        cached ??
        (yield* this.fetchEmailByIdSerializedEffect(id, options.fresh ?? false));
      const endedAt = yield* Clock.currentTimeMillis;
      yield* this.logger.info(
        `Email read completed source=${cached ? "cache" : "imap"} results=${email ? 1 : 0} elapsedMs=${endedAt - startedAt}`,
      );
      return email;
    });
  }

  saveSentCopyEffect(
    input: SentCopyInput,
    options: {
      allowAppend?: boolean;
      beforeAppend?: Effect.Effect<
        boolean,
        unknown,
        import("@micthiesen/mitools/docstore").Docstore | Logger
      >;
    } = {},
  ) {
    return this.runSerializedEffect(
      "IMAP save Sent copy",
      Effect.gen({ self: this }, function* () {
        const client = yield* this.requireClientEffect;
        const result = yield* appendSentCopyEffect(client, input, options);
        this.clearReadCaches();
        return result;
      }).pipe(Effect.ensuring(this.restoreInboxEffect())),
    );
  }

  createDraftEffect(input: EmailDraftInput, options: { allowAppend?: boolean } = {}) {
    return this.runSerializedEffect(
      "IMAP create draft",
      Effect.gen({ self: this }, function* () {
        const client = yield* this.requireClientEffect;
        const from = getComposeEmailConfiguration()?.from ?? this.auth.user;
        return yield* appendDraftEffect(client, input, from, options);
      }).pipe(Effect.ensuring(this.restoreInboxEffect())),
    );
  }

  private fetchEmailByIdSerializedEffect(id: string, fresh: boolean) {
    return this.runSerializedEffect(
      "IMAP fetch by id",
      Effect.gen({ self: this }, function* () {
        const now = yield* Clock.currentTimeMillis;
        const cached = fresh ? undefined : this.directGetCache.get(id, now);
        if (cached) return cached;
        const client = yield* this.requireClientEffect;
        const coords = decodeMessageId(id);
        const folderNames = coords ? [coords.folder] : [...FOLDERS];
        for (const folder of folderNames) {
          if (coords && coords.folder !== folder) continue;
          const email = yield* this.findInFolderEffect(
            client,
            folder,
            id,
            coords,
            fresh,
          );
          if (email) {
            this.directGetCache.set(
              id,
              email,
              estimateEmailBytes(email),
              yield* Clock.currentTimeMillis,
            );
            return email;
          }
          // Discover Sent only after the usual folders miss; LIST outages cannot block Inbox reads.
          if (!coords && folder === FOLDERS[FOLDERS.length - 1]) {
            const sent = (yield* this.promiseEffect(
              "discover Sent for direct read",
              () => client.list(),
            )).filter((folder) => folder.specialUse?.toLowerCase() === "\\sent");
            if (sent.length === 1 && !folderNames.includes(sent[0].path))
              folderNames.push(sent[0].path);
          }
        }
        return undefined;
      }).pipe(Effect.ensuring(this.restoreInboxEffect())),
    );
  }

  searchEmailsEffect(options: EmailSearchOptions) {
    return Effect.gen({ self: this }, function* () {
      const startedAt = yield* Clock.currentTimeMillis;
      if (options.fresh) this.searchResultCache.delete(searchCacheKey(options));
      const cached = options.fresh
        ? undefined
        : this.searchResultCache.get(searchCacheKey(options), startedAt);
      const emails = cached ?? (yield* this.searchSerializedEffect(options));
      const endedAt = yield* Clock.currentTimeMillis;
      yield* this.logger.info(
        `Email search completed source=${cached ? "cache" : "imap"} results=${emails.length} elapsedMs=${endedAt - startedAt}`,
      );
      return emails;
    });
  }

  inspectArchiveMessageEffect(identity: ArchiveIdentity) {
    return this.runSerializedEffect(
      "inspect archive source",
      Effect.gen({ self: this }, function* () {
        const client = yield* this.requireClientEffect;
        return yield* inspectArchiveSourceEffect(client, identity);
      }).pipe(Effect.ensuring(this.restoreInboxEffect())),
    );
  }

  moveArchiveMessageEffect(identity: ArchiveIdentity, sourceHash: string) {
    return this.runSerializedEffect(
      "archive exact Inbox message",
      Effect.gen({ self: this }, function* () {
        const client = yield* this.requireClientEffect;
        return yield* nativeMoveArchiveMessageEffect(client, identity, sourceHash);
      }).pipe(
        Effect.ensuring(Effect.sync(() => this.clearReadCaches())),
        Effect.ensuring(this.restoreInboxEffect()),
      ),
    );
  }

  reconcileArchiveMessageEffect(identity: ArchiveIdentity, sourceHash: string) {
    return this.runSerializedEffect(
      "reconcile archive message",
      Effect.gen({ self: this }, function* () {
        const client = yield* this.requireClientEffect;
        return yield* readArchiveMoveEffect(client, identity, sourceHash);
      }).pipe(Effect.ensuring(this.restoreInboxEffect())),
    );
  }

  restoreArchiveMessageEffect(
    identity: ArchiveIdentity,
    destination: ArchiveLocation,
    sourceHash: string,
  ) {
    return this.runSerializedEffect(
      "restore exact archived message",
      Effect.gen({ self: this }, function* () {
        const client = yield* this.requireClientEffect;
        return yield* nativeRestoreArchiveMessageEffect(
          client,
          identity,
          destination,
          sourceHash,
        );
      }).pipe(
        Effect.ensuring(Effect.sync(() => this.clearReadCaches())),
        Effect.ensuring(this.restoreInboxEffect()),
      ),
    );
  }

  inspectArchiveDestinationEffect(
    identity: ArchiveIdentity,
    destination: ArchiveLocation,
  ) {
    return this.runSerializedEffect(
      "inspect archive restore source",
      Effect.gen({ self: this }, function* () {
        const client = yield* this.requireClientEffect;
        return yield* inspectNativeArchiveDestinationEffect(
          client,
          identity,
          destination,
        );
      }).pipe(Effect.ensuring(this.restoreInboxEffect())),
    );
  }

  verifyArchiveLocationEffect(
    location: ArchiveLocation,
    messageId: string,
    sourceHash: string,
    flags: readonly string[],
  ) {
    return this.runSerializedEffect(
      "verify archive location",
      Effect.gen({ self: this }, function* () {
        const client = yield* this.requireClientEffect;
        return yield* verifyNativeArchiveLocationEffect(
          client,
          location,
          messageId,
          sourceHash,
          flags,
        );
      }).pipe(Effect.ensuring(this.restoreInboxEffect())),
    );
  }

  reconcileRestoreMessageEffect(
    identity: ArchiveIdentity,
    destination: ArchiveLocation,
    sourceHash: string,
  ) {
    return this.runSerializedEffect(
      "reconcile restore message",
      Effect.gen({ self: this }, function* () {
        const client = yield* this.requireClientEffect;
        return yield* readRestoreMoveEffect(client, identity, destination, sourceHash);
      }).pipe(Effect.ensuring(this.restoreInboxEffect())),
    );
  }

  copyExactArchiveMessageEffect(
    source: ArchiveIdentity,
    targetFolder: string,
    snapshot: ArchiveSnapshot,
  ) {
    return this.runSerializedEffect(
      "UID COPY exact email",
      Effect.gen({ self: this }, function* () {
        const client = yield* this.requireClientEffect;
        return yield* copyExactMessageEffect(client, source, targetFolder, snapshot);
      }).pipe(
        Effect.ensuring(Effect.sync(() => this.clearReadCaches())),
        Effect.ensuring(this.restoreInboxEffect()),
      ),
    );
  }

  reconcileExactCopyEffect(
    source: ArchiveIdentity,
    targetFolder: string,
    snapshot: ArchiveSnapshot,
  ) {
    return this.runSerializedEffect(
      "reconcile UID COPY",
      Effect.gen({ self: this }, function* () {
        const client = yield* this.requireClientEffect;
        return yield* readExactCopyEffect(client, source, targetFolder, snapshot);
      }).pipe(Effect.ensuring(this.restoreInboxEffect())),
    );
  }

  markExactArchiveSourceDeletedEffect(
    source: ArchiveIdentity,
    destination: ArchiveLocation,
    snapshot: ArchiveSnapshot,
  ) {
    return this.runSerializedEffect(
      "mark exact source Deleted",
      Effect.gen({ self: this }, function* () {
        const client = yield* this.requireClientEffect;
        return yield* markExactDeletedEffect(client, source, destination, snapshot);
      }).pipe(
        Effect.ensuring(Effect.sync(() => this.clearReadCaches())),
        Effect.ensuring(this.restoreInboxEffect()),
      ),
    );
  }

  inspectExactDeletedSourceEffect(
    source: ArchiveIdentity,
    destination: ArchiveLocation,
    snapshot: ArchiveSnapshot,
  ) {
    return this.runSerializedEffect(
      "inspect exact Deleted source",
      Effect.gen({ self: this }, function* () {
        const client = yield* this.requireClientEffect;
        return yield* readExactDeletedEffect(client, source, destination, snapshot);
      }).pipe(Effect.ensuring(this.restoreInboxEffect())),
    );
  }

  expungeExactArchiveSourceEffect(
    source: ArchiveIdentity,
    destination: ArchiveLocation,
    snapshot: ArchiveSnapshot,
  ) {
    return this.runSerializedEffect(
      "UID EXPUNGE exact source",
      Effect.gen({ self: this }, function* () {
        const client = yield* this.requireClientEffect;
        const archiveClient = client as typeof client & {
          uidExpungeExact(uid: number): Promise<boolean>;
        };
        archiveClient.uidExpungeExact = (uid) => uidExpungeExact(client, uid);
        return yield* expungeExactSourceEffect(
          archiveClient,
          source,
          destination,
          snapshot,
        );
      }).pipe(
        Effect.ensuring(Effect.sync(() => this.clearReadCaches())),
        Effect.ensuring(this.restoreInboxEffect()),
      ),
    );
  }

  private searchSerializedEffect(options: EmailSearchOptions) {
    return this.runSerializedEffect(
      "IMAP search",
      Effect.gen({ self: this }, function* () {
        const cacheKey = searchCacheKey(options);
        const cacheNow = yield* Clock.currentTimeMillis;
        const cached = options.fresh
          ? undefined
          : this.searchResultCache.get(cacheKey, cacheNow);
        if (cached) return cached;
        const client = yield* this.requireClientEffect;
        const folderNames =
          options.folder === "inbox"
            ? ["INBOX"]
            : options.folder === "archive"
              ? ["Archive"]
              : options.folder === "sent"
                ? (yield* this.promiseEffect("discover Sent mailbox", () =>
                    client.list(),
                  ))
                    .filter((folder) => folder.specialUse?.toLowerCase() === "\\sent")
                    .map((folder) => folder.path)
                : FOLDERS;
        if (options.folder === "sent" && folderNames.length !== 1)
          return yield* new ImapOperationError({
            operation: "discover Sent mailbox",
            cause: new Error("Expected exactly one server-designated Sent mailbox"),
          });
        const perFolderLimit = Math.min(Math.max(1, options.limit), 50);
        const byFolder = yield* Effect.forEach(
          folderNames,
          (folder) =>
            Effect.acquireUseRelease(
              this.promiseEffect(`lock ${folder}`, () =>
                client.getMailboxLock(folder, { readOnly: true }),
              ),
              () =>
                Effect.gen({ self: this }, function* () {
                  const criteria = {
                    ...(options.query ||
                    options.from ||
                    options.to ||
                    options.subject ||
                    options.unread !== undefined ||
                    options.since ||
                    options.before
                      ? {}
                      : { all: true }),
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
                  const readNow = yield* Clock.currentTimeMillis;
                  // Keep selected snapshots alive even if the subsequent batch
                  // evicts them from the bounded shared cache.
                  const cachedByUid = new Map<number, FetchedEmail>();
                  if (!options.fresh) {
                    for (const uid of uids) {
                      const cached = this.parsedMessageCache.get(
                        messageCacheKey(folder, mailboxValidity, uid),
                        readNow,
                      );
                      if (cached) cachedByUid.set(uid, cached);
                    }
                  }
                  const uncached = uids.filter((uid) => !cachedByUid.has(uid));
                  const fetchedByUid = new Map<number, FetchedEmail>();
                  if (uncached.length > 0) {
                    // One FETCH command streams the selected UIDs. Parse each
                    // source before reading the next so attachment buffers do
                    // not accumulate for the whole result set.
                    yield* Stream.fromAsyncIterable(
                      client.fetch(
                        uncached,
                        { source: true, internalDate: true },
                        { uid: true },
                      ),
                      (cause) =>
                        new ImapOperationError({
                          operation: `fetch ${uncached.length} messages`,
                          cause,
                        }),
                    ).pipe(
                      Stream.runForEach((full) =>
                        Effect.gen({ self: this }, function* () {
                          if (!full.source) return;
                          const parsed = yield* this.promiseEffect(
                            "parse message",
                            () => simpleParser(full.source!),
                          );
                          const email = mapParsedMessage(
                            parsed,
                            { folder, uidValidity: mailboxValidity, uid: full.uid },
                            toDate(full.internalDate),
                          );
                          fetchedByUid.set(full.uid, email);
                          this.parsedMessageCache.set(
                            messageCacheKey(folder, mailboxValidity, full.uid),
                            email,
                            estimateEmailBytes(email),
                            yield* Clock.currentTimeMillis,
                          );
                          yield* this.logger.debug(
                            `Email read parsed uid=${full.uid} folder=${folder} bytes=${full.source.length}`,
                          );
                        }),
                      ),
                    );
                  }
                  return uids
                    .map((uid) => fetchedByUid.get(uid) ?? cachedByUid.get(uid))
                    .filter((email): email is FetchedEmail => email !== undefined);
                }),
              (lock) => Effect.sync(() => lock.release()),
            ),
          { concurrency: 1 },
        );

        const result = byFolder
          .flat()
          .sort((a, b) => Date.parse(b.receivedAt) - Date.parse(a.receivedAt))
          .slice(0, options.limit);
        const completedAt = yield* Clock.currentTimeMillis;
        this.searchResultCache.set(
          cacheKey,
          result,
          estimateEmailBytes(result),
          completedAt,
        );
        return result;
      }).pipe(Effect.ensuring(this.restoreInboxEffect())),
    );
  }

  private findInFolderEffect(
    client: ImapFlow,
    folder: string,
    id: string,
    coords: (MessageCoords & { index?: number }) | undefined,
    fresh: boolean,
  ) {
    return Effect.acquireUseRelease(
      this.promiseEffect(`lock ${folder}`, () =>
        client.getMailboxLock(folder, { readOnly: true }),
      ),
      () =>
        Effect.gen({ self: this }, function* () {
          const mailboxValidity = String(orUndefined(client.mailbox)?.uidValidity);

          let uid: number | undefined;
          if (coords) {
            if (coords.uidValidity !== mailboxValidity) return undefined;
            uid = coords.uid;
          } else {
            const now = yield* Clock.currentTimeMillis;
            const located = fresh ? undefined : this.messageLocationCache.get(id, now);
            if (located?.folder === folder && located.uidValidity === mailboxValidity) {
              const exists = orUndefined(
                yield* this.promiseEffect(`check uid ${located.uid}`, () =>
                  client.fetchOne(String(located.uid), { uid: true }, { uid: true }),
                ),
              );
              if (exists) uid = located.uid;
              else this.messageLocationCache.delete(id);
            } else if (located) {
              this.messageLocationCache.delete(id);
            }
            if (uid === undefined) {
              const found = yield* this.promiseEffect(`find message ${id}`, () =>
                client.search({ header: { "message-id": id } }, { uid: true }),
              );
              if (Array.isArray(found) && found.length > 0) {
                if (found.length > 50)
                  return yield* new ImapOperationError({
                    operation: "find exact Message-ID",
                    cause: new Error(
                      "Too many substring matches for a bounded direct read",
                    ),
                  });
                for (const candidate of [...found].reverse()) {
                  const email = yield* this.fetchMappedMessageEffect(
                    client,
                    folder,
                    mailboxValidity,
                    candidate,
                    undefined,
                    fresh,
                  );
                  if (email?.messageId !== id) continue;
                  if (!fresh)
                    this.messageLocationCache.set(
                      id,
                      { folder, uidValidity: mailboxValidity, uid: candidate },
                      id.length + folder.length + mailboxValidity.length + 16,
                      yield* Clock.currentTimeMillis,
                    );
                  return email;
                }
                return undefined;
              }
            }
          }
          if (uid === undefined) return undefined;

          const cacheKey = messageCacheKey(folder, mailboxValidity, uid);
          const cacheNow = yield* Clock.currentTimeMillis;
          const cached = fresh
            ? undefined
            : this.parsedMessageCache.get(cacheKey, cacheNow);
          if (cached) {
            const exists = orUndefined(
              yield* this.promiseEffect(`check uid ${uid}`, () =>
                client.fetchOne(String(uid), { uid: true }, { uid: true }),
              ),
            );
            if (exists && (coords || cached.messageId === id)) return cached;
            this.parsedMessageCache.delete(cacheKey);
            return undefined;
          }
          const email = yield* this.fetchMappedMessageEffect(
            client,
            folder,
            mailboxValidity,
            uid,
            undefined,
            fresh,
          );
          // HEADER SEARCH is substring-based; never return another message as a reply parent.
          if (!coords && email?.messageId !== id) {
            this.messageLocationCache.delete(id);
            return undefined;
          }
          return email;
        }),
      (lock) => Effect.sync(() => lock.release()),
    );
  }

  fetchAttachmentByIdEffect(
    messageId: string,
    attachmentId: string,
    options: { maxBytes?: number } = {},
  ) {
    return this.runSerializedEffect(
      "IMAP stable attachment download",
      Effect.gen({ self: this }, function* () {
        const maxBytes = options.maxBytes ?? MAX_ATTACHMENT_BYTES;
        if (
          !validAttachmentMessageId(messageId) ||
          !/^imap-attachment:[a-f0-9]{64}$/.test(attachmentId) ||
          !Number.isInteger(maxBytes) ||
          maxBytes < 1 ||
          maxBytes > MAX_ATTACHMENT_BYTES
        )
          return yield* new ImapOperationError({
            operation: "validate attachment request",
            cause: new Error("Invalid attachment identity or byte limit"),
          });
        const client = yield* this.requireClientEffect;
        const folders = [...FOLDERS];
        for (const folder of folders) {
          const result = yield* Effect.acquireUseRelease(
            this.promiseEffect("lock attachment mailbox", () =>
              client.getMailboxLock(folder, { readOnly: true }),
            ),
            () =>
              Effect.gen({ self: this }, function* () {
                const found = yield* this.promiseEffect("find attachment message", () =>
                  client.search({ header: { "message-id": messageId } }, { uid: true }),
                );
                if (!Array.isArray(found)) return undefined;
                if (found.length > 50)
                  return yield* new ImapOperationError({
                    operation: "find attachment message",
                    cause: new Error(
                      "Too many matches for bounded attachment retrieval",
                    ),
                  });
                for (const uid of [...found].reverse()) {
                  const meta = orUndefined(
                    yield* this.promiseEffect("preflight attachment message", () =>
                      client.fetchOne(
                        String(uid),
                        { size: true, envelope: true },
                        { uid: true },
                      ),
                    ),
                  );
                  if (!meta || meta.envelope?.messageId !== messageId) continue;
                  if (
                    !Number.isSafeInteger(meta.size) ||
                    meta.size! < 1 ||
                    meta.size! > MAX_ATTACHMENT_MESSAGE_BYTES
                  )
                    return yield* new ImapOperationError({
                      operation: "bound attachment message",
                      cause: new Error(
                        "Message exceeds attachment retrieval byte limit",
                      ),
                    });
                  const full = orUndefined(
                    yield* this.promiseEffect("fetch bounded attachment message", () =>
                      client.fetchOne(
                        String(uid),
                        {
                          source: {
                            start: 0,
                            maxLength: MAX_ATTACHMENT_MESSAGE_BYTES + 1,
                          },
                        },
                        { uid: true },
                      ),
                    ),
                  );
                  if (!full?.source) continue;
                  if (full.source.length > MAX_ATTACHMENT_MESSAGE_BYTES)
                    return yield* new ImapOperationError({
                      operation: "bound attachment source",
                      cause: new Error(
                        "Message exceeds attachment retrieval byte limit",
                      ),
                    });
                  const parsed = yield* this.promiseEffect(
                    "parse bounded attachment message",
                    () =>
                      simpleParser(full.source!, {
                        skipHtmlToText: true,
                        skipTextToHtml: true,
                        skipImageLinks: true,
                      }),
                  );
                  if (parsed.messageId !== messageId) continue;
                  const matches = parsed.attachments.filter((part) => {
                    const partId = attachmentPartId(part);
                    return (
                      partId !== undefined &&
                      encodeStableAttachmentId(messageId, partId) === attachmentId
                    );
                  });
                  if (matches.length === 0) continue;
                  if (matches.length !== 1)
                    return yield* new ImapOperationError({
                      operation: "validate MIME attachment identity",
                      cause: new Error("Ambiguous MIME attachment identity"),
                    });
                  const part = matches[0];
                  if (!Buffer.isBuffer(part.content) || part.content.length > maxBytes)
                    return yield* new ImapOperationError({
                      operation: "bound attachment bytes",
                      cause: new Error("Attachment exceeds requested byte limit"),
                    });
                  return {
                    name: safeAttachmentFilename(part.filename),
                    mimeType: declaredAttachmentMimeType(part),
                    data: part.content,
                  };
                }
                return undefined;
              }),
            (lock) => Effect.sync(() => lock.release()),
          );
          if (result) return result;
          if (folder === FOLDERS[FOLDERS.length - 1]) {
            const sent = (yield* this.promiseEffect(
              "discover Sent attachment mailbox",
              () => client.list(),
            )).filter((mailbox) => mailbox.specialUse?.toLowerCase() === "\\sent");
            if (sent.length === 1 && !folders.includes(sent[0].path))
              folders.push(sent[0].path);
          }
        }
        return undefined;
      }).pipe(Effect.ensuring(this.restoreInboxEffect())),
    );
  }

  downloadAttachmentEffect(attachment: EmailAttachment) {
    return this.runSerializedEffect(
      "IMAP attachment download",
      Effect.gen({ self: this }, function* () {
        const target = decodeAttachmentBlobId(attachment.blobId);
        if (!target) {
          yield* this.logger.warn(
            `Unrecognized attachment handle: ${attachment.blobId}`,
          );
          return undefined;
        }

        const client = yield* this.requireClientEffect;
        return yield* Effect.acquireUseRelease(
          this.promiseEffect(`lock ${target.folder}`, () =>
            client.getMailboxLock(target.folder, { readOnly: true }),
          ),
          () =>
            Effect.gen({ self: this }, function* () {
              const mailboxValidity = String(orUndefined(client.mailbox)?.uidValidity);
              if (mailboxValidity !== target.uidValidity) {
                yield* this.logger.warn(
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
                yield* this.logger.warn(
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
                yield* this.logger.warn(
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
  private restoreInboxEffect() {
    return Effect.suspend(() => {
      const client = this.client;
      if (!client?.usable || orUndefined(client.mailbox)?.path === "INBOX")
        return Effect.void;
      return this.promiseEffect("reselect INBOX", () =>
        client.mailboxOpen("INBOX", { readOnly: true }),
      ).pipe(
        Effect.catch((error) =>
          this.logger.debug(`Failed to reselect INBOX: ${error.message}`),
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
    fresh = false,
  ) {
    return Effect.gen({ self: this }, function* () {
      const cacheKey = messageCacheKey(folder, uidValidity, uid);
      const cacheNow = yield* Clock.currentTimeMillis;
      const cached = fresh
        ? undefined
        : this.parsedMessageCache.get(cacheKey, cacheNow);
      if (cached) return cached;
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
      const parsedNow = yield* Clock.currentTimeMillis;
      this.parsedMessageCache.set(
        cacheKey,
        email,
        estimateEmailBytes(email),
        parsedNow,
      );
      yield* this.logger.debug(
        `Email read parsed uid=${uid} folder=${folder} attachments=${email.attachments.length}`,
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

  private runSerializedEffect<A, E, R>(
    operation: string,
    effect: Effect.Effect<A, E, R>,
  ): Effect.Effect<A, ImapOperationError, R> {
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
): Effect.Effect<
  { metas: Array<{ uid: number; internalDate?: Date }>; complete: boolean },
  ImapOperationError
> {
  // ImapFlow exposes message ranges only as an async iterable. This adapter is
  // the single Promise boundary for consuming that library protocol.
  return Effect.acquireUseRelease(
    Effect.sync(() =>
      client
        .fetch(`${fromUid}:*`, { internalDate: true }, { uid: true })
        [Symbol.asyncIterator](),
    ),
    (iterator) =>
      Effect.tryPromise({
        try: async () => {
          const metas: Array<{ uid: number; internalDate?: Date }> = [];
          let complete = true;
          for (;;) {
            const next = await iterator.next();
            if (next.done) break;
            const message = next.value;
            if (message.uid >= fromUid) {
              metas.push({
                uid: message.uid,
                internalDate: toDate(message.internalDate),
              });
            }
            if (metas.length > MAX_EMAILS_PER_PASS) {
              complete = false;
              break;
            }
          }
          metas.sort((a, b) => a.uid - b.uid);
          return { metas, complete };
        },
        catch: (cause) =>
          new ImapOperationError({ operation: `scan ${folder}`, cause }),
      }),
    (iterator) => {
      const close = iterator.return?.();
      return close ? Effect.tryPromise(() => close).pipe(Effect.ignore) : Effect.void;
    },
  );
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

function messageCacheKey(folder: string, validity: string, uid: number): string {
  return `${folder}\0${validity}\0${uid}`;
}

function searchCacheKey(options: EmailSearchOptions): string {
  return JSON.stringify({
    query: options.query?.trim() ?? "",
    from: options.from?.trim() ?? "",
    to: options.to?.trim() ?? "",
    subject: options.subject?.trim() ?? "",
    unread: options.unread ?? null,
    since: options.since?.toISOString() ?? null,
    before: options.before?.toISOString() ?? null,
    folder: options.folder ?? "all",
    limit: options.limit,
  });
}

function estimateEmailBytes(value: unknown): number {
  try {
    return Buffer.byteLength(JSON.stringify(value));
  } catch {
    return Number.POSITIVE_INFINITY;
  }
}
