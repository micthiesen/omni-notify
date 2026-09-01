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
  list(): Promise<readonly AutoReadMailbox[]>;
  getMailboxLock(path: string, options?: { readOnly?: boolean }): Promise<AutoReadLock>;
  search(
    query: { seen: false; since: Date },
    options: { uid: true },
  ): Promise<number[] | false>;
  messageFlagsAdd(
    range: number[],
    flags: string[],
    options: { uid: true; silent: true },
  ): Promise<boolean>;
}

export interface AutoReadLogger {
  warn(message: string): void;
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

export function discoverAutoReadFoldersEffect(
  client: AutoReadClient,
): Effect.Effect<string[], AutoReadError> {
  return Effect.tryPromise({
    try: () => client.list(),
    catch: (cause) => new AutoReadError({ operation: "list mailboxes", cause }),
  }).pipe(Effect.map(selectAutoReadFolders));
}

/** Mark recent unread messages read, continuing when an individual folder fails. */
export function markRecentUnreadReadEffect(
  client: AutoReadClient,
  folders: readonly string[],
  logger: AutoReadLogger,
): Effect.Effect<void> {
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
            Effect.tryPromise({
              try: () => client.search({ seen: false, since }, { uid: true }),
              catch: (cause) =>
                new AutoReadError({ operation: `search ${folder}`, cause }),
            }).pipe(
              Effect.flatMap((found) => {
                if (Array.isArray(found) && found.length > 0) {
                  return Effect.tryPromise({
                    try: () =>
                      client.messageFlagsAdd(found, ["\\Seen"], {
                        uid: true,
                        silent: true,
                      }),
                    catch: (cause) =>
                      new AutoReadError({ operation: `mark ${folder}`, cause }),
                  });
                }
                return Effect.void;
              }),
            ),
          (lock) => Effect.sync(() => lock.release()),
        ).pipe(
          Effect.catchAll((error) =>
            Effect.sync(() =>
              logger.warn(
                `IMAP auto-read failed for folder "${folder}": ${error.message}`,
              ),
            ),
          ),
        ),
      { concurrency: 1, discard: true },
    );
  });
}
import { Clock, Data, Effect } from "effect";
