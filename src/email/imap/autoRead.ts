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

export async function discoverAutoReadFolders(
  client: AutoReadClient,
): Promise<string[]> {
  return selectAutoReadFolders(await client.list());
}

/** Mark recent unread messages read, continuing when an individual folder fails. */
export async function markRecentUnreadRead(
  client: AutoReadClient,
  folders: readonly string[],
  logger: AutoReadLogger,
): Promise<void> {
  const since = new Date(Date.now() - AUTO_READ_AGE_MS);

  for (const folder of folders) {
    try {
      const lock = await client.getMailboxLock(folder, { readOnly: false });
      try {
        const found = await client.search({ seen: false, since }, { uid: true });
        if (Array.isArray(found) && found.length > 0) {
          await client.messageFlagsAdd(found, ["\\Seen"], {
            uid: true,
            silent: true,
          });
        }
      } finally {
        lock.release();
      }
    } catch (error) {
      logger.warn(
        `IMAP auto-read failed for folder "${folder}": ${errorMessage(error)}`,
      );
    }
  }
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
