const AUTO_READ_AGE_MS = 24 * 60 * 60_000;
const AUTO_READ_ROLES = new Set(["\\Archive", "\\Junk", "\\Trash"]);

export interface AutoReadMailbox {
  path: string;
  flags?: ReadonlySet<string>;
  specialUse?: string;
}

export interface AutoReadLock {
  release(): void;
}

export interface AutoReadClient {
  mailbox?: { uidValidity: bigint | number | string } | false;
  list(): Promise<readonly AutoReadMailbox[]>;
  getMailboxLock(path: string, options?: { readOnly?: boolean }): Promise<AutoReadLock>;
  search(
    query: { seen: false; since: Date } | { header: { "Message-ID": string } },
    options: { uid: true },
  ): Promise<number[] | false>;
  fetchOne?(
    uid: string,
    query: { envelope: true },
    options: { uid: true },
  ): Promise<{ envelope?: { messageId?: string } } | false>;
  messageFlagsAdd(
    range: number[],
    flags: string[],
    options: { uid: true; silent: true },
  ): Promise<boolean>;
}

export interface AutoReadProtection {
  skip: boolean;
  excludedUids: ReadonlySet<number>;
  fallbackMessageIds?: readonly string[];
}

export interface AutoReadLogger<R = never> {
  warn(message: string): Effect.Effect<void, never, R>;
}

export class AutoReadError extends Data.TaggedError("AutoReadError")<{
  readonly operation: string;
  readonly cause: unknown;
}> {
  public override get message(): string {
    return this.cause instanceof Error ? this.cause.message : String(this.cause);
  }
}

/**
 * Select only the server-designated Archive, Junk, and Trash mailboxes.
 * Special-use roles are authoritative, so localized mailbox paths work too.
 */
export function selectAutoReadFolders(mailboxes: readonly AutoReadMailbox[]): string[] {
  const paths = new Set<string>();
  for (const mailbox of mailboxes) {
    if (mailbox.flags?.has("\\Noselect")) continue;
    if (!mailbox.specialUse || !AUTO_READ_ROLES.has(mailbox.specialUse)) continue;
    paths.add(mailbox.path);
  }
  return [...paths];
}

export function selectAutoReadArchiveFolders(
  mailboxes: readonly AutoReadMailbox[],
): string[] {
  return [
    ...new Set(
      mailboxes
        .filter(
          (mailbox) =>
            mailbox.specialUse === "\\Archive" && !mailbox.flags?.has("\\Noselect"),
        )
        .map((mailbox) => mailbox.path),
    ),
  ];
}

export function discoverAutoReadMailboxPlanEffect(client: AutoReadClient) {
  return Effect.tryPromise({
    try: () => client.list(),
    catch: (cause) => new AutoReadError({ operation: "list mailboxes", cause }),
  }).pipe(
    Effect.map((mailboxes) => ({
      folders: selectAutoReadFolders(mailboxes),
      archiveFolders: selectAutoReadArchiveFolders(mailboxes),
    })),
  );
}

export function discoverAutoReadFoldersEffect(
  client: AutoReadClient,
): Effect.Effect<string[], AutoReadError> {
  return Effect.tryPromise({
    try: () => client.list(),
    catch: (cause) => new AutoReadError({ operation: "list mailboxes", cause }),
  }).pipe(Effect.map(selectAutoReadFolders));
}

/** Mark recent unread messages read, continuing when an individual folder fails. */
export function markRecentUnreadReadEffect<R, P = never>(
  client: AutoReadClient,
  folders: readonly string[],
  logger: AutoReadLogger<R>,
  protection?: (
    folder: string,
    uidValidity: string | undefined,
  ) => Effect.Effect<AutoReadProtection, unknown, P>,
) {
  return Effect.gen(function* () {
    const now = yield* Clock.currentTimeMillis;
    const since = new Date(now - AUTO_READ_AGE_MS);

    yield* Effect.forEach(
      folders,
      (folder) =>
        Effect.acquireUseRelease(
          Effect.tryPromise({
            try: () => client.getMailboxLock(folder, { readOnly: false }),
            catch: (cause) => new AutoReadError({ operation: `lock ${folder}`, cause }),
          }),
          () =>
            Effect.gen(function* () {
              const validity = client.mailbox
                ? String(client.mailbox.uidValidity)
                : undefined;
              const policy = protection
                ? yield* protection(folder, validity)
                : { skip: false, excludedUids: new Set<number>() };
              if (policy.skip) return;
              const found = yield* Effect.tryPromise({
                try: () => client.search({ seen: false, since }, { uid: true }),
                catch: (cause) =>
                  new AutoReadError({ operation: `search ${folder}`, cause }),
              });
              if (found === false)
                return yield* new AutoReadError({
                  operation: `search ${folder}`,
                  cause: new Error("Unread search was not confirmed"),
                });
              if (found.length === 0) return;
              const excluded = new Set(policy.excludedUids);
              for (const messageId of policy.fallbackMessageIds ?? []) {
                const candidates = yield* Effect.tryPromise({
                  try: () =>
                    client.search(
                      { header: { "Message-ID": messageId } },
                      { uid: true },
                    ),
                  catch: (cause) =>
                    new AutoReadError({
                      operation: `find protected ${folder} message`,
                      cause,
                    }),
                });
                if (candidates === false || candidates.length > 50)
                  return yield* new AutoReadError({
                    operation: `find protected ${folder} message`,
                    cause: new Error(
                      "Protected message lookup was not bounded and confirmed",
                    ),
                  });
                if (!client.fetchOne)
                  return yield* new AutoReadError({
                    operation: `verify protected ${folder} message`,
                    cause: new Error("Exact header read unavailable"),
                  });
                for (const uid of candidates) {
                  const candidate = yield* Effect.tryPromise({
                    try: () =>
                      client.fetchOne!(String(uid), { envelope: true }, { uid: true }),
                    catch: (cause) =>
                      new AutoReadError({
                        operation: `verify protected ${folder} message`,
                        cause,
                      }),
                  });
                  if (!candidate)
                    return yield* new AutoReadError({
                      operation: `verify protected ${folder} message`,
                      cause: new Error("Candidate UID vanished"),
                    });
                  if (candidate.envelope?.messageId === messageId) excluded.add(uid);
                }
              }
              const toMark = found.filter((uid) => !excluded.has(uid));
              if (toMark.length === 0) return;
              const marked = yield* Effect.tryPromise({
                try: () =>
                  client.messageFlagsAdd(toMark, ["\\Seen"], {
                    uid: true,
                    silent: true,
                  }),
                catch: (cause) =>
                  new AutoReadError({ operation: `mark ${folder}`, cause }),
              });
              if (!marked)
                return yield* new AutoReadError({
                  operation: `mark ${folder}`,
                  cause: new Error("Flag update was not confirmed"),
                });
            }),
          (lock) => Effect.sync(() => lock.release()),
        ).pipe(
          Effect.catch((error) =>
            logger.warn(
              `IMAP auto-read failed for folder "${folder}": ${error instanceof Error ? error.message : String(error)}`,
            ),
          ),
        ),
      { concurrency: 1, discard: true },
    );
  });
}
import { Clock, Data, Effect } from "effect";
