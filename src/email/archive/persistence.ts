import { createHash } from "node:crypto";
import { decodeDoc, Docstore } from "@micthiesen/mitools/docstore";
import { Clock, Data, Effect, Option, Schema } from "effect";
import type {
  ArchiveIdentity,
  ArchiveLocation,
  ArchiveSnapshot,
} from "../imap/archive.js";

export type ArchiveActionStatus =
  | "queued"
  | "cancelled"
  | "claimed"
  | "archived"
  | "uncertain"
  | "failed"
  | "restore_claimed"
  | "restored"
  | "restore_uncertain"
  | "copy_claimed"
  | "copy_verified"
  | "delete_claimed"
  | "expunge_claimed"
  | "copied_source_retained"
  | "restore_copy_claimed"
  | "restore_copy_verified"
  | "restore_delete_claimed"
  | "restore_expunge_claimed"
  | "restore_copied_source_retained";

export interface ArchiveAction {
  actionId: string;
  identity: ArchiveIdentity;
  status: ArchiveActionStatus;
  snapshot?: ArchiveSnapshot;
  restoreSnapshot?: ArchiveSnapshot;
  destination?: ArchiveLocation;
  restoredLocation?: ArchiveLocation;
  attempts: number;
  nextAttemptAt: number;
  reason?:
    | "transport_unavailable"
    | "source_unavailable"
    | "native_move_unavailable"
    | "safe_move_unavailable"
    | "verification_failed"
    | "uncertain"
    | "copy_uncertain"
    | "copied_source_retained"
    | "copied_source_deleted"
    | "source_mark_uncertain"
    | "source_expunge_uncertain";
  createdAt: number;
  updatedAt: number;
}

const IdentitySchema = Schema.Struct({
  folder: Schema.String,
  uidValidity: Schema.String,
  uid: Schema.Number,
  messageId: Schema.String,
});
const LocationSchema = Schema.Struct({
  folder: Schema.String,
  uidValidity: Schema.String,
  uid: Schema.Number,
});
const SnapshotSchema = Schema.Struct({
  sourceHash: Schema.String,
  flags: Schema.Array(Schema.String),
  strategy: Schema.optional(Schema.Literal("uidplus_copy")),
  targetFolder: Schema.optional(Schema.String),
});
const ActionSchema = Schema.Struct({
  actionId: Schema.String,
  identity: IdentitySchema,
  status: Schema.Literals([
    "queued",
    "cancelled",
    "claimed",
    "archived",
    "uncertain",
    "failed",
    "restore_claimed",
    "restored",
    "restore_uncertain",
    "copy_claimed",
    "copy_verified",
    "delete_claimed",
    "expunge_claimed",
    "copied_source_retained",
    "restore_copy_claimed",
    "restore_copy_verified",
    "restore_delete_claimed",
    "restore_expunge_claimed",
    "restore_copied_source_retained",
  ]),
  snapshot: Schema.optional(SnapshotSchema),
  restoreSnapshot: Schema.optional(SnapshotSchema),
  destination: Schema.optional(LocationSchema),
  restoredLocation: Schema.optional(LocationSchema),
  attempts: Schema.Number,
  nextAttemptAt: Schema.Number,
  reason: Schema.optional(
    Schema.Literals([
      "transport_unavailable",
      "source_unavailable",
      "native_move_unavailable",
      "safe_move_unavailable",
      "verification_failed",
      "uncertain",
      "copy_uncertain",
      "copied_source_retained",
      "copied_source_deleted",
      "source_mark_uncertain",
      "source_expunge_uncertain",
    ]),
  ),
  createdAt: Schema.Number,
  updatedAt: Schema.Number,
});

export class ArchiveActionError extends Data.TaggedError("ArchiveActionError")<{
  readonly message: string;
  readonly cause?: unknown;
}> {}

const digest = (value: string) => createHash("sha256").update(value).digest("hex");
const actionKey = (actionId: string) => `email-archive:action:${actionId}`;
const markerKey = (messageId: string) => `email-archive:message:${digest(messageId)}`;
const historyKey = (messageId: string) => `email-archive:history:${digest(messageId)}`;

function decodeAction(data: Buffer): ArchiveAction {
  return Schema.decodeUnknownSync(ActionSchema)(decodeDoc<unknown>(data));
}

/**
 * Every action ever reserved for a Message-ID. Rows written before the history
 * key existed only have the latest-action marker, which is read as a one-item
 * history.
 */
function reservationIds(
  history: Buffer | undefined,
  marker: Buffer | undefined,
): string[] {
  const ids = history
    ? Schema.decodeUnknownSync(Schema.Array(Schema.String))(decodeDoc<unknown>(history))
    : [];
  const latest = marker
    ? [Schema.decodeUnknownSync(Schema.String)(decodeDoc<unknown>(marker))]
    : [];
  return [...new Set([...ids, ...latest])];
}

/** The exact physical copy: mailbox, UIDVALIDITY, and UID. One account per runtime. */
function sameSourceCopy(left: ArchiveIdentity, right: ArchiveIdentity): boolean {
  return (
    left.folder === right.folder &&
    left.uidValidity === right.uidValidity &&
    left.uid === right.uid
  );
}

const SETTLED_STATUSES: ReadonlySet<ArchiveActionStatus> = new Set([
  "archived",
  "restored",
  "cancelled",
  "failed",
]);

/** No mailbox operation is pending or unproven for this action. */
export const isSettledArchiveStatus = (status: ArchiveActionStatus) =>
  SETTLED_STATUSES.has(status);

/**
 * A Message-ID can name several physical copies. Each copy is reserved by its
 * own source identity, while any unsettled action for the same Message-ID
 * blocks a sibling: recovery reconciles by Message-ID and content hash, so two
 * concurrent moves could make each other's outcome ambiguous.
 */
function reservationConflict(
  previous: ArchiveAction,
  identity: ArchiveIdentity,
): string | undefined {
  if (sameSourceCopy(previous.identity, identity))
    return previous.status === "restored" ||
      previous.status === "cancelled" ||
      previous.status === "failed"
      ? undefined
      : "Message already has an archive action";
  return isSettledArchiveStatus(previous.status)
    ? undefined
    : "Another copy with this Message-ID has an unresolved archive action";
}

export const archiveActionId = (idempotencyKey: string) => digest(idempotencyKey);

export const queueArchiveActionEffect = Effect.fn("EmailArchive.queue")(function* (
  idempotencyKey: string,
  identity: ArchiveIdentity,
) {
  if (
    identity.folder !== "INBOX" ||
    !Number.isSafeInteger(identity.uid) ||
    identity.uid < 1 ||
    !/^<[^<>\s\x00-\x1f\x7f]+>$/.test(identity.messageId)
  )
    return yield* new ArchiveActionError({ message: "Invalid exact Inbox identity" });
  const docstore = yield* Docstore;
  const now = yield* Clock.currentTimeMillis;
  const actionId = archiveActionId(idempotencyKey);
  const result = yield* docstore.transaction(
    "queue email archive",
    (tx): ArchiveAction | { error: string } => {
      const existing = tx.getRawRow(actionKey(actionId), now);
      if (existing) {
        const action = decodeAction(existing.data);
        if (
          action.identity.folder !== identity.folder ||
          action.identity.uidValidity !== identity.uidValidity ||
          action.identity.uid !== identity.uid ||
          action.identity.messageId !== identity.messageId
        )
          return { error: "Idempotency key belongs to a different message" };
        return action;
      }
      const marker = tx.getRawRow(markerKey(identity.messageId), now);
      const historyRow = tx.getRawRow(historyKey(identity.messageId), now);
      const history = reservationIds(historyRow?.data, marker?.data);
      for (const previousId of history) {
        const previous = tx.getRawRow(actionKey(previousId), now);
        if (!previous) continue;
        const conflict = reservationConflict(decodeAction(previous.data), identity);
        if (conflict) return { error: conflict };
      }
      const action: ArchiveAction = {
        actionId,
        identity,
        status: "queued",
        attempts: 0,
        nextAttemptAt: now,
        createdAt: now,
        updatedAt: now,
      };
      tx.upsertDoc(
        actionKey(actionId),
        action,
        { entity: "email-archive-action" },
        now,
      );
      tx.upsertDoc(
        markerKey(identity.messageId),
        actionId,
        { entity: "email-archive-message" },
        now,
      );
      tx.upsertDoc(
        historyKey(identity.messageId),
        [...new Set([...history, actionId])],
        { entity: "email-archive-history" },
        now,
      );
      return action;
    },
  );
  if ("error" in result)
    return yield* new ArchiveActionError({ message: result.error });
  return result;
});

export const getArchiveActionEffect = Effect.fn("EmailArchive.get")(function* (
  actionId: string,
) {
  const docstore = yield* Docstore;
  const row = yield* docstore.getRawRow(actionKey(actionId));
  return Option.isSome(row) ? decodeAction(row.value.data) : undefined;
});

export const listArchiveActionsEffect = Effect.fn("EmailArchive.list")(function* () {
  const docstore = yield* Docstore;
  const rows = yield* docstore.getRawRowsByPrefix("email-archive:action:");
  return rows.map((row) => decodeAction(row.data));
});

export const updateArchiveActionEffect = Effect.fn("EmailArchive.update")(function* (
  actionId: string,
  expected: ArchiveActionStatus,
  status: ArchiveActionStatus,
  patch: Partial<
    Pick<
      ArchiveAction,
      | "snapshot"
      | "restoreSnapshot"
      | "destination"
      | "restoredLocation"
      | "attempts"
      | "nextAttemptAt"
      | "reason"
    >
  > = {},
) {
  const docstore = yield* Docstore;
  const now = yield* Clock.currentTimeMillis;
  const result = yield* docstore.transaction(
    "update email archive",
    (tx): ArchiveAction | { error: string } => {
      const row = tx.getRawRow(actionKey(actionId), now);
      if (!row) return { error: "Archive action not found" };
      const previous = decodeAction(row.data);
      if (previous.status !== expected)
        return { error: `Archive action is ${previous.status}, expected ${expected}` };
      const updated: ArchiveAction = { ...previous, ...patch, status, updatedAt: now };
      tx.upsertDoc(
        actionKey(actionId),
        updated,
        { entity: "email-archive-action" },
        now,
      );
      return updated;
    },
  );
  if ("error" in result)
    return yield* new ArchiveActionError({ message: result.error });
  return result;
});

function sameLocation(
  left: ArchiveLocation | undefined,
  right: ArchiveLocation | undefined,
): boolean {
  return (
    !!left &&
    !!right &&
    left.folder === right.folder &&
    left.uidValidity === right.uidValidity &&
    left.uid === right.uid
  );
}

/** All actions recorded for a Message-ID, across every physical copy. */
export const listArchiveActionsForMessageEffect = Effect.fn(
  "EmailArchive.listForMessage",
)(function* (messageId: string) {
  const docstore = yield* Docstore;
  const history = yield* docstore.getRawRow(historyKey(messageId));
  const marker = yield* docstore.getRawRow(markerKey(messageId));
  const actions: ArchiveAction[] = [];
  for (const actionId of reservationIds(
    Option.isSome(history) ? history.value.data : undefined,
    Option.isSome(marker) ? marker.value.data : undefined,
  )) {
    const action = yield* getArchiveActionEffect(actionId);
    if (action) actions.push(action);
  }
  return actions;
});

/** Suppress only a mailbox event that a recorded action's own move caused. */
export const isArchiveActionMessageEffect = Effect.fn("EmailArchive.isActionMessage")(
  function* (messageId: string, origin?: ArchiveLocation) {
    if (!origin) return false;
    const actions = yield* listArchiveActionsForMessageEffect(messageId);
    // Inbox copies known to predate a restore: every action's source copy and
    // any earlier completed restore. A restored copy gets a UID above them.
    const knownInboxUids = actions.flatMap((action) =>
      [
        action.identity,
        action.status === "restored" ? action.restoredLocation : undefined,
      ]
        .filter(
          (location): location is ArchiveLocation =>
            location?.folder === "INBOX" && location.uidValidity === origin.uidValidity,
        )
        .map((location) => location.uid),
    );
    const newestKnownInboxUid = Math.max(0, ...knownInboxUids);
    for (const action of actions) {
      if (sameLocation(origin, action.identity)) continue;
      if (
        action.status === "queued" ||
        action.status === "cancelled" ||
        action.status === "failed"
      )
        continue;
      if (
        sameLocation(origin, action.destination) ||
        sameLocation(origin, action.restoredLocation)
      )
        return true;
      if (action.status === "claimed" || action.status === "uncertain") {
        if (origin.folder !== "INBOX") return true;
      }
      if (
        action.status === "restore_claimed" ||
        action.status === "restore_uncertain"
      ) {
        // Without a recorded restore UID, suppress only Inbox UIDs above every
        // known copy. An unrecorded same-Message-ID copy above them is
        // indistinguishable from this restore until reconciliation records it.
        const preexistingCopy =
          !!action.restoredLocation || origin.uid <= newestKnownInboxUid;
        if (origin.folder === "INBOX" && !preexistingCopy) return true;
      }
    }
    return false;
  },
);

/** Keep action-owned archived mail unread when the ordinary Archive sweep runs. */
export const archiveAutoReadProtectionEffect = Effect.fn(
  "EmailArchive.autoReadProtection",
)(function* (archiveFolder: string, uidValidity: string | undefined) {
  const actions = yield* listArchiveActionsEffect();
  const excludedUids = new Set<number>();
  const fallbackMessageIds = new Set<string>();
  for (const action of actions) {
    if (
      action.status === "claimed" ||
      action.status === "uncertain" ||
      action.status === "copy_claimed" ||
      action.status === "copy_verified" ||
      action.status === "delete_claimed" ||
      action.status === "expunge_claimed" ||
      action.status === "copied_source_retained" ||
      action.status === "restore_claimed" ||
      action.status === "restore_uncertain" ||
      action.status === "restore_copy_claimed" ||
      action.status === "restore_copy_verified" ||
      action.status === "restore_delete_claimed" ||
      action.status === "restore_expunge_claimed" ||
      action.status === "restore_copied_source_retained"
    ) {
      fallbackMessageIds.add(action.identity.messageId);
      continue;
    }
    if (action.status !== "archived") continue;
    const destination = action.destination;
    if (
      !destination ||
      destination.folder !== archiveFolder ||
      !uidValidity ||
      destination.uidValidity !== uidValidity
    ) {
      fallbackMessageIds.add(action.identity.messageId);
      continue;
    }
    excludedUids.add(destination.uid);
  }
  return { skip: false, excludedUids, fallbackMessageIds: [...fallbackMessageIds] };
});
