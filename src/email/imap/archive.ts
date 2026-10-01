import { createHash } from "node:crypto";
import { Data, Effect } from "effect";

export interface ArchiveLocation {
  folder: string;
  uidValidity: string;
  uid: number;
}

export interface ArchiveIdentity extends ArchiveLocation {
  messageId: string;
}

export interface ArchiveSnapshot {
  sourceHash: string;
  flags: readonly string[];
  strategy?: "uidplus_copy";
  targetFolder?: string;
}

export interface ArchiveMoveResult extends ArchiveSnapshot {
  destination: ArchiveLocation;
}

export type ArchiveReconcileResult =
  | { state: "moved"; destination: ArchiveLocation; snapshot: ArchiveSnapshot }
  | { state: "not_moved" }
  | { state: "uncertain" };

export interface ArchiveClient {
  capabilities: Map<string, boolean | number>;
  mailbox?: { path: string; uidValidity: bigint | number | string } | false;
  list(): Promise<Array<{ path: string; specialUse?: string; flags?: Set<string> }>>;
  getMailboxLock(
    path: string,
    options: { readOnly: boolean },
  ): Promise<{ release(): void }>;
  fetchOne(
    uid: string,
    query: { source: true; flags: true; envelope: true },
    options: { uid: true },
  ): Promise<
    | {
        source?: Buffer;
        flags?: Set<string>;
        envelope?: { messageId?: string };
      }
    | false
  >;
  search(
    query: { header: { "Message-ID": string } },
    options: { uid: true },
  ): Promise<number[] | false>;
  messageMove(
    uid: number[],
    destination: string,
    options: { uid: true },
  ): Promise<
    | {
        uidValidity?: bigint;
        uidMap?: Map<number, number>;
      }
    | false
  >;
  messageCopy?(
    uid: number[],
    destination: string,
    options: { uid: true },
  ): Promise<{ uidValidity?: bigint; uidMap?: Map<number, number> } | false>;
  messageFlagsAdd?(
    uid: number[],
    flags: string[],
    options: { uid: true },
  ): Promise<boolean>;
  /** Narrow adapter over ImapFlow's run/exec; never plain EXPUNGE. */
  uidExpungeExact?(uid: number): Promise<boolean>;
}

export class ArchiveImapError extends Data.TaggedError("ArchiveImapError")<{
  readonly operation: string;
  readonly cause: unknown;
}> {
  public override get message(): string {
    const detail =
      this.cause instanceof Error ? this.cause.message : String(this.cause);
    return `${this.operation} failed: ${detail}`;
  }
}

const attempt = <A>(operation: string, run: () => Promise<A>) =>
  Effect.tryPromise({
    try: run,
    catch: (cause) => new ArchiveImapError({ operation, cause }),
  });

const fail = (operation: string, detail: string) =>
  new ArchiveImapError({ operation, cause: new Error(detail) });

const contentHash = (source: Buffer) =>
  createHash("sha256").update(source).digest("hex");
const MAX_ARCHIVE_SOURCE_BYTES = 20 * 1024 * 1024;

function selectedValidity(client: ArchiveClient): string | undefined {
  return client.mailbox ? String(client.mailbox.uidValidity) : undefined;
}

function normalizedFlags(flags: Set<string>): string[] {
  // \\Recent is session-local and cannot be preserved across mailboxes.
  return [...flags].filter((flag) => flag.toLowerCase() !== "\\recent").sort();
}

function readExactEffect(
  client: ArchiveClient,
  identity: ArchiveIdentity,
): Effect.Effect<ArchiveSnapshot | undefined, ArchiveImapError> {
  return Effect.gen(function* () {
    if (selectedValidity(client) !== identity.uidValidity)
      return yield* fail("verify mailbox identity", "UIDVALIDITY changed");
    const message = yield* attempt("read exact message", () =>
      client.fetchOne(
        String(identity.uid),
        { source: true, flags: true, envelope: true },
        { uid: true },
      ),
    );
    if (!message) return undefined;
    if (message.envelope?.messageId !== identity.messageId)
      return yield* fail(
        "verify message identity",
        "UID belongs to a different Message-ID",
      );
    if (!message.source || !message.flags)
      return yield* fail("read exact message", "MIME source or flags unavailable");
    if (message.source.byteLength > MAX_ARCHIVE_SOURCE_BYTES)
      return yield* fail("read exact message", "MIME source exceeds archive limit");
    return {
      sourceHash: contentHash(message.source),
      flags: normalizedFlags(message.flags),
    };
  });
}

function inMailboxEffect<A, E, R>(
  client: ArchiveClient,
  folder: string,
  readOnly: boolean,
  use: () => Effect.Effect<A, E, R>,
): Effect.Effect<A, ArchiveImapError | E, R> {
  return Effect.acquireUseRelease(
    attempt(`select ${folder}`, () => client.getMailboxLock(folder, { readOnly })),
    use,
    (lock) => Effect.sync(() => lock.release()),
  );
}

export function discoverArchiveEffect(client: ArchiveClient) {
  return Effect.gen(function* () {
    const folders = yield* attempt("discover Archive", () => client.list());
    const archives = folders.filter(
      (folder) =>
        folder.specialUse?.toLowerCase() === "\\archive" &&
        !folder.flags?.has("\\Noselect"),
    );
    if (archives.length !== 1)
      return yield* fail(
        "discover Archive",
        "Expected one selectable designated Archive",
      );
    if (archives[0].path === "INBOX")
      return yield* fail("discover Archive", "Archive resolves to Inbox");
    return archives[0].path;
  });
}

export function inspectArchiveSourceEffect(
  client: ArchiveClient,
  identity: ArchiveIdentity,
) {
  return Effect.gen(function* () {
    if (identity.folder !== "INBOX")
      return yield* fail("inspect archive source", "Only Inbox mail can be archived");
    if (!client.capabilities.has("MOVE") && !client.capabilities.has("UIDPLUS"))
      return yield* fail("safe move", "Server advertises neither MOVE nor UIDPLUS");
    const archive = yield* discoverArchiveEffect(client);
    const snapshot = yield* inMailboxEffect(client, "INBOX", true, () =>
      Effect.gen(function* () {
        const snapshot = yield* readExactEffect(client, identity);
        if (!snapshot)
          return yield* fail("inspect archive source", "Inbox UID is gone");
        return snapshot;
      }),
    );
    if (
      !client.capabilities.has("MOVE") &&
      snapshot.flags.some((flag) => flag.toLowerCase() === "\\deleted")
    )
      return yield* fail("inspect archive source", "Inbox source is already Deleted");
    const existing = yield* findExactInFolderEffect(
      client,
      archive,
      identity.messageId,
      snapshot.sourceHash,
    );
    if (existing.length > 0)
      return yield* fail(
        "inspect archive destination",
        "Matching Archive copy already exists",
      );
    return client.capabilities.has("MOVE")
      ? snapshot
      : { ...snapshot, strategy: "uidplus_copy" as const, targetFolder: archive };
  });
}

function moveExactEffect(
  client: ArchiveClient,
  source: ArchiveIdentity,
  destinationFolder: string,
  expectedHash: string,
) {
  return Effect.gen(function* () {
    // ImapFlow emulates MOVE with COPY + \\Deleted/EXPUNGE without this extension.
    if (!client.capabilities.has("MOVE"))
      return yield* fail("native MOVE", "Server does not advertise MOVE");
    return yield* inMailboxEffect(client, source.folder, false, () =>
      Effect.gen(function* () {
        const snapshot = yield* readExactEffect(client, source);
        if (!snapshot || snapshot.sourceHash !== expectedHash)
          return yield* fail(
            "verify source before MOVE",
            "Source changed or disappeared",
          );
        const moved = yield* attempt("native MOVE", () =>
          client.messageMove([source.uid], destinationFolder, { uid: true }),
        );
        if (!moved) return yield* fail("native MOVE", "Server did not confirm MOVE");
        const uid = moved.uidMap?.get(source.uid);
        if (!uid || !moved.uidValidity)
          return yield* fail(
            "verify MOVE",
            "Server supplied no destination UID mapping",
          );
        const destination: ArchiveLocation = {
          folder: destinationFolder,
          uidValidity: String(moved.uidValidity),
          uid,
        };
        return { destination, ...snapshot };
      }),
    );
  });
}

export function moveArchiveMessageEffect(
  client: ArchiveClient,
  identity: ArchiveIdentity,
  expectedHash: string,
) {
  return Effect.gen(function* () {
    if (identity.folder !== "INBOX")
      return yield* fail("archive message", "Only Inbox mail can be archived");
    const snapshot = yield* inspectArchiveSourceEffect(client, identity);
    if (snapshot.sourceHash !== expectedHash)
      return yield* fail("archive message", "Source changed since reservation");
    const archive = yield* discoverArchiveEffect(client);
    return yield* moveExactEffect(client, identity, archive, expectedHash);
  });
}

export function restoreArchiveMessageEffect(
  client: ArchiveClient,
  original: ArchiveIdentity,
  destination: ArchiveLocation,
  expectedHash: string,
) {
  return Effect.gen(function* () {
    if (original.folder !== "INBOX")
      return yield* fail("restore archive message", "Original mailbox was not Inbox");
    const archive = yield* discoverArchiveEffect(client);
    if (destination.folder !== archive)
      return yield* fail(
        "restore archive message",
        "Recorded destination is not Archive",
      );
    const existingInbox = yield* findExactInFolderEffect(
      client,
      "INBOX",
      original.messageId,
      expectedHash,
    );
    if (existingInbox.length > 0)
      return yield* fail(
        "restore archive message",
        "Matching Inbox copy already exists",
      );
    return yield* moveExactEffect(
      client,
      { ...destination, messageId: original.messageId },
      "INBOX",
      expectedHash,
    );
  });
}

/** Check the recorded Archive UID without changing Seen or any other flag. */
export function inspectArchiveDestinationEffect(
  client: ArchiveClient,
  original: ArchiveIdentity,
  destination: ArchiveLocation,
) {
  return Effect.gen(function* () {
    const archive = yield* discoverArchiveEffect(client);
    if (destination.folder !== archive)
      return yield* fail(
        "inspect restore source",
        "Recorded destination is not Archive",
      );
    if (!client.capabilities.has("MOVE") && !client.capabilities.has("UIDPLUS"))
      return yield* fail(
        "restore archive message",
        "Neither MOVE nor UIDPLUS is available",
      );
    return yield* inMailboxEffect(client, archive, true, () =>
      Effect.gen(function* () {
        const snapshot = yield* readExactEffect(client, {
          ...destination,
          messageId: original.messageId,
        });
        if (!snapshot)
          return yield* fail("inspect restore source", "Archived UID is gone");
        if (
          !client.capabilities.has("MOVE") &&
          snapshot.flags.some((flag) => flag.toLowerCase() === "\\deleted")
        )
          return yield* fail(
            "inspect restore source",
            "Archived UID is already Deleted",
          );
        return client.capabilities.has("MOVE")
          ? snapshot
          : { ...snapshot, strategy: "uidplus_copy" as const, targetFolder: "INBOX" };
      }),
    );
  });
}

function findExactInFolderEffect(
  client: ArchiveClient,
  folder: string,
  messageId: string,
  expectedHash: string,
) {
  return inMailboxEffect(client, folder, true, () =>
    Effect.gen(function* () {
      const validity = selectedValidity(client);
      if (!validity) return yield* fail("inspect mailbox", "UIDVALIDITY unavailable");
      const candidates = yield* attempt("find exact Message-ID", () =>
        client.search({ header: { "Message-ID": messageId } }, { uid: true }),
      );
      if (candidates === false)
        return yield* fail("find exact Message-ID", "IMAP search was not confirmed");
      if (candidates && candidates.length > 50)
        return yield* fail("find exact Message-ID", "Too many candidate messages");
      const matches: Array<{
        destination: ArchiveLocation;
        snapshot: ArchiveSnapshot;
      }> = [];
      for (const uid of candidates || []) {
        const message = yield* attempt("read Message-ID candidate", () =>
          client.fetchOne(
            String(uid),
            { source: true, flags: true, envelope: true },
            { uid: true },
          ),
        );
        if (!message)
          return yield* fail("read Message-ID candidate", "Candidate UID vanished");
        // IMAP HEADER search is substring matching; only this mismatch is safe to skip.
        if (message.envelope?.messageId !== messageId) continue;
        if (!message.source || !message.flags)
          return yield* fail(
            "read Message-ID candidate",
            "MIME source or flags unavailable",
          );
        if (message.source.byteLength > MAX_ARCHIVE_SOURCE_BYTES)
          return yield* fail(
            "read Message-ID candidate",
            "MIME source exceeds archive limit",
          );
        const snapshot = {
          sourceHash: contentHash(message.source),
          flags: normalizedFlags(message.flags),
        };
        if (snapshot.sourceHash === expectedHash)
          matches.push({
            destination: { folder, uidValidity: validity, uid },
            snapshot,
          });
      }
      return matches;
    }),
  );
}

/** No mutation: reconcile a possibly lost MOVE response after a restart. */
export function reconcileArchiveMessageEffect(
  client: ArchiveClient,
  identity: ArchiveIdentity,
  sourceHash: string,
): Effect.Effect<ArchiveReconcileResult, ArchiveImapError> {
  return Effect.gen(function* () {
    const archive = yield* discoverArchiveEffect(client);
    const sourceResult = yield* inMailboxEffect(client, "INBOX", true, () =>
      readExactEffect(client, identity),
    ).pipe(Effect.result);
    if (sourceResult._tag === "Failure") return { state: "uncertain" } as const;
    const source = sourceResult.success;
    const destinations = yield* findExactInFolderEffect(
      client,
      archive,
      identity.messageId,
      sourceHash,
    );
    if (source?.sourceHash === sourceHash && destinations.length === 0)
      return { state: "not_moved" } as const;
    if (!source && destinations.length === 1)
      return { state: "moved", ...destinations[0] } as const;
    return { state: "uncertain" } as const;
  });
}

/** Read only verification of the exact UID returned by COPYUID after MOVE. */
export function verifyArchiveLocationEffect(
  client: ArchiveClient,
  location: ArchiveLocation,
  messageId: string,
  sourceHash: string,
  expectedFlags: readonly string[],
) {
  return inMailboxEffect(client, location.folder, true, () =>
    Effect.gen(function* () {
      const snapshot = yield* readExactEffect(client, { ...location, messageId });
      return (
        snapshot?.sourceHash === sourceHash &&
        JSON.stringify(snapshot.flags) === JSON.stringify([...expectedFlags].sort())
      );
    }),
  );
}

/** After a lost restore response, inspect both folders without repeating MOVE. */
export function reconcileRestoreMessageEffect(
  client: ArchiveClient,
  original: ArchiveIdentity,
  destination: ArchiveLocation,
  sourceHash: string,
): Effect.Effect<ArchiveReconcileResult, ArchiveImapError> {
  return Effect.gen(function* () {
    const archive = yield* discoverArchiveEffect(client);
    if (destination.folder !== archive)
      return yield* fail("reconcile restore", "Recorded destination is not Archive");
    const archivedResult = yield* inMailboxEffect(client, archive, true, () =>
      readExactEffect(client, { ...destination, messageId: original.messageId }),
    ).pipe(Effect.result);
    if (archivedResult._tag === "Failure") return { state: "uncertain" } as const;
    const archived = archivedResult.success;
    const inbox = yield* findExactInFolderEffect(
      client,
      "INBOX",
      original.messageId,
      sourceHash,
    );
    if (!archived && inbox.length === 1)
      return { state: "moved", ...inbox[0] } as const;
    if (archived?.sourceHash === sourceHash && inbox.length === 0)
      return { state: "not_moved" } as const;
    return { state: "uncertain" } as const;
  });
}

/** Only the UIDPLUS path uses these steps. Each call makes at most one mutation. */
export function copyExactArchiveMessageEffect(
  client: ArchiveClient,
  source: ArchiveIdentity,
  targetFolder: string,
  snapshot: ArchiveSnapshot,
) {
  return Effect.gen(function* () {
    const archive = yield* discoverArchiveEffect(client);
    if (
      !(
        (source.folder === "INBOX" && targetFolder === archive) ||
        (source.folder === archive && targetFolder === "INBOX")
      )
    )
      return yield* fail("UID COPY", "Source or target is outside the archive action");
    if (!client.capabilities.has("UIDPLUS") || !client.messageCopy)
      return yield* fail("UID COPY", "UIDPLUS COPY is unavailable");
    const existing = yield* findExactInFolderEffect(
      client,
      targetFolder,
      source.messageId,
      snapshot.sourceHash,
    );
    if (existing.length)
      return yield* fail("UID COPY", "Matching destination already exists");
    return yield* inMailboxEffect(client, source.folder, false, () =>
      Effect.gen(function* () {
        const current = yield* readExactEffect(client, source);
        if (
          !current ||
          current.sourceHash !== snapshot.sourceHash ||
          JSON.stringify(current.flags) !== JSON.stringify(snapshot.flags) ||
          current.flags.some((flag) => flag.toLowerCase() === "\\deleted")
        )
          return yield* fail("UID COPY", "Source changed or is already Deleted");
        const copied = yield* attempt("UID COPY", () =>
          client.messageCopy!([source.uid], targetFolder, { uid: true }),
        );
        if (!copied) return yield* fail("UID COPY", "Server did not confirm COPY");
        const uid = copied.uidMap?.get(source.uid);
        if (!uid || !copied.uidValidity)
          return yield* fail("UID COPY", "Server supplied no COPYUID mapping");
        return { folder: targetFolder, uidValidity: String(copied.uidValidity), uid };
      }),
    );
  });
}

/** Read-only recovery after a lost COPY response. Never issues a second COPY. */
export function reconcileExactCopyEffect(
  client: ArchiveClient,
  source: ArchiveIdentity,
  targetFolder: string,
  snapshot: ArchiveSnapshot,
): Effect.Effect<ArchiveReconcileResult, ArchiveImapError> {
  return Effect.gen(function* () {
    const sourceResult = yield* inMailboxEffect(client, source.folder, true, () =>
      readExactEffect(client, source),
    ).pipe(Effect.result);
    if (
      sourceResult._tag === "Failure" ||
      sourceResult.success?.sourceHash !== snapshot.sourceHash
    )
      return { state: "uncertain" } as const;
    const matches = yield* findExactInFolderEffect(
      client,
      targetFolder,
      source.messageId,
      snapshot.sourceHash,
    );
    if (
      matches.length !== 1 ||
      JSON.stringify(matches[0].snapshot.flags) !== JSON.stringify(snapshot.flags)
    )
      return { state: "uncertain" } as const;
    return { state: "moved", ...matches[0] } as const;
  });
}

/** Verify both copies, then mark only the selected source UID Deleted. */
export function markExactArchiveSourceDeletedEffect(
  client: ArchiveClient,
  source: ArchiveIdentity,
  destination: ArchiveLocation,
  snapshot: ArchiveSnapshot,
) {
  return Effect.gen(function* () {
    const archive = yield* discoverArchiveEffect(client);
    if (
      !(
        (source.folder === "INBOX" && destination.folder === archive) ||
        (source.folder === archive && destination.folder === "INBOX")
      )
    )
      return yield* fail("UID STORE", "Archive mailbox identity changed");
    if (!client.capabilities.has("UIDPLUS") || !client.messageFlagsAdd)
      return yield* fail("UID STORE", "UIDPLUS STORE is unavailable");
    const verified = yield* verifyArchiveLocationEffect(
      client,
      destination,
      source.messageId,
      snapshot.sourceHash,
      snapshot.flags,
    );
    if (!verified) return yield* fail("UID STORE", "Verified destination changed");
    return yield* inMailboxEffect(client, source.folder, false, () =>
      Effect.gen(function* () {
        const current = yield* readExactEffect(client, source);
        if (
          !current ||
          current.sourceHash !== snapshot.sourceHash ||
          JSON.stringify(current.flags) !== JSON.stringify(snapshot.flags) ||
          current.flags.some((flag) => flag.toLowerCase() === "\\deleted")
        )
          return yield* fail("UID STORE", "Exact source changed before deletion");
        const marked = yield* attempt("UID STORE Deleted", () =>
          client.messageFlagsAdd!([source.uid], ["\\Deleted"], { uid: true }),
        );
        if (!marked)
          return yield* fail("UID STORE", "Server did not confirm Deleted flag");
        return true;
      }),
    );
  });
}

/** A lost STORE response is reconciled by reading the exact UID and flags. */
export function inspectExactDeletedSourceEffect(
  client: ArchiveClient,
  source: ArchiveIdentity,
  destination: ArchiveLocation,
  snapshot: ArchiveSnapshot,
) {
  return Effect.gen(function* () {
    const verified = yield* verifyArchiveLocationEffect(
      client,
      destination,
      source.messageId,
      snapshot.sourceHash,
      snapshot.flags,
    );
    if (!verified) return "uncertain" as const;
    const result = yield* inMailboxEffect(client, source.folder, true, () =>
      readExactEffect(client, source),
    ).pipe(Effect.result);
    if (result._tag === "Failure") return "uncertain" as const;
    const current = result.success;
    if (!current) return "absent" as const;
    if (current.sourceHash !== snapshot.sourceHash) return "uncertain" as const;
    const otherFlags = current.flags.filter(
      (flag) => flag.toLowerCase() !== "\\deleted",
    );
    if (JSON.stringify(otherFlags) !== JSON.stringify(snapshot.flags))
      return "uncertain" as const;
    return current.flags.some((flag) => flag.toLowerCase() === "\\deleted")
      ? ("marked" as const)
      : ("unmarked" as const);
  });
}

/** Only UID EXPUNGE of the exact source UID; no mailbox-wide EXPUNGE. */
export function expungeExactArchiveSourceEffect(
  client: ArchiveClient,
  source: ArchiveIdentity,
  destination: ArchiveLocation,
  snapshot: ArchiveSnapshot,
) {
  return Effect.gen(function* () {
    const archive = yield* discoverArchiveEffect(client);
    if (
      !(
        (source.folder === "INBOX" && destination.folder === archive) ||
        (source.folder === archive && destination.folder === "INBOX")
      )
    )
      return yield* fail("UID EXPUNGE", "Archive mailbox identity changed");
    if (!client.capabilities.has("UIDPLUS") || !client.uidExpungeExact)
      return yield* fail("UID EXPUNGE", "Exact UID EXPUNGE unavailable");
    const observed = yield* inspectExactDeletedSourceEffect(
      client,
      source,
      destination,
      snapshot,
    );
    if (observed !== "marked")
      return yield* fail("UID EXPUNGE", "Source or destination changed");
    return yield* inMailboxEffect(client, source.folder, false, () =>
      Effect.gen(function* () {
        // Re-read under the mutation lock to reject UIDVALIDITY change/reuse.
        const current = yield* readExactEffect(client, source);
        if (
          !current ||
          current.sourceHash !== snapshot.sourceHash ||
          !current.flags.some((flag) => flag.toLowerCase() === "\\deleted") ||
          JSON.stringify(
            current.flags.filter((flag) => flag.toLowerCase() !== "\\deleted"),
          ) !== JSON.stringify(snapshot.flags)
        )
          return yield* fail("UID EXPUNGE", "Exact source changed");
        const removed = yield* attempt("UID EXPUNGE", () =>
          client.uidExpungeExact!(source.uid),
        );
        if (!removed)
          return yield* fail("UID EXPUNGE", "Server did not confirm UID EXPUNGE");
        return true;
      }),
    );
  });
}
