import { Clock, Effect, Semaphore } from "effect";
import type { TaskServices } from "../../task-runs/registry.js";
import type { EmailTransport } from "../types.js";
import type { ArchiveLocation, ArchiveSourceRequest } from "../imap/archive.js";
import { ArchiveActionError, type ArchiveAction } from "./persistence.js";
import {
  getArchiveActionEffect,
  isSettledArchiveStatus,
  listArchiveActionsEffect,
  listArchiveActionsForMessageEffect,
  updateArchiveActionEffect,
} from "./persistence.js";

type Transport = EmailTransport<unknown, TaskServices>;
const archiveWorkflowSemaphore = Semaphore.makeUnsafe(1);

/** Serialize a whole archive workflow, including the receipt write after IMAP COPY. */
export const withArchiveActionWorkflowEffect = <A, E, R>(
  effect: Effect.Effect<A, E, R>,
) => archiveWorkflowSemaphore.withPermits(1)(effect);

function archiveTransport(transport: Transport) {
  if (
    !transport.inspectArchiveMessageEffect ||
    !transport.moveArchiveMessageEffect ||
    !transport.reconcileArchiveMessageEffect ||
    !transport.restoreArchiveMessageEffect ||
    !transport.inspectArchiveDestinationEffect ||
    !transport.reconcileRestoreMessageEffect ||
    !transport.verifyArchiveLocationEffect
  )
    return undefined;
  return {
    inspect: transport.inspectArchiveMessageEffect.bind(transport),
    move: transport.moveArchiveMessageEffect.bind(transport),
    reconcile: transport.reconcileArchiveMessageEffect.bind(transport),
    restore: transport.restoreArchiveMessageEffect.bind(transport),
    inspectDestination: transport.inspectArchiveDestinationEffect.bind(transport),
    reconcileRestore: transport.reconcileRestoreMessageEffect.bind(transport),
    verify: transport.verifyArchiveLocationEffect.bind(transport),
  };
}

function copyTransport(transport: Transport) {
  if (
    !transport.copyExactArchiveMessageEffect ||
    !transport.reconcileExactCopyEffect ||
    !transport.markExactArchiveSourceDeletedEffect ||
    !transport.inspectExactDeletedSourceEffect ||
    !transport.expungeExactArchiveSourceEffect
  )
    return undefined;
  return {
    copy: transport.copyExactArchiveMessageEffect.bind(transport),
    reconcileCopy: transport.reconcileExactCopyEffect.bind(transport),
    mark: transport.markExactArchiveSourceDeletedEffect.bind(transport),
    inspect: transport.inspectExactDeletedSourceEffect.bind(transport),
    expunge: transport.expungeExactArchiveSourceEffect.bind(transport),
  };
}

function inspectFailureReason(cause: unknown): ArchiveAction["reason"] {
  const message = cause instanceof Error ? cause.message : "";
  if (message.includes("does not advertise MOVE")) return "native_move_unavailable";
  if (message.includes("neither MOVE nor UIDPLUS")) return "safe_move_unavailable";
  if (
    message.includes("Inbox UID is gone") ||
    message.includes("Inbox source is already Deleted") ||
    message.includes("UIDVALIDITY changed") ||
    message.includes("different Message-ID")
  )
    return "source_unavailable";
  return "transport_unavailable";
}

function blocksQueuedSibling(sibling: ArchiveAction, action: ArchiveAction): boolean {
  if (sibling.actionId === action.actionId || isSettledArchiveStatus(sibling.status))
    return false;
  if (sibling.status !== "queued") return true;
  return (
    sibling.createdAt < action.createdAt ||
    (sibling.createdAt === action.createdAt && sibling.actionId < action.actionId)
  );
}

/** The forward source plus verified Archive copies owned by settled siblings. */
const archiveSourceEffect = Effect.fn("EmailArchive.source")(function* (
  action: ArchiveAction,
) {
  const siblings = yield* listArchiveActionsForMessageEffect(action.identity.messageId);
  const claimedCopies = siblings.flatMap((sibling) =>
    sibling.actionId !== action.actionId &&
    sibling.status === "archived" &&
    sibling.destination
      ? [sibling.destination]
      : [],
  );
  return { ...action.identity, claimedCopies } satisfies ArchiveSourceRequest;
});

export const processArchiveActionEffect = Effect.fn("EmailArchive.process")(function* (
  action: ArchiveAction,
  transport: Transport,
) {
  if (action.status !== "queued") return action;
  const api = archiveTransport(transport);
  if (!api)
    return yield* new ArchiveActionError({
      message: "Active email transport does not support archive actions",
    });
  // Queueing runs outside the workflow lock, so a sibling restore may have
  // started since. Wait for it to settle. Among queued siblings the oldest
  // proceeds, so two queued copies never wait on each other.
  const siblings = yield* listArchiveActionsForMessageEffect(action.identity.messageId);
  if (siblings.some((sibling) => blocksQueuedSibling(sibling, action))) {
    const now = yield* Clock.currentTimeMillis;
    return yield* updateArchiveActionEffect(action.actionId, "queued", "queued", {
      nextAttemptAt: now + 60_000,
    });
  }
  const source = yield* archiveSourceEffect(action);
  const inspected = yield* api.inspect(source).pipe(Effect.result);
  if (inspected._tag === "Failure") {
    const reason = inspectFailureReason(inspected.failure);
    const attempts = action.attempts + 1;
    const terminal = reason !== "transport_unavailable" || attempts >= 5;
    const now = yield* Clock.currentTimeMillis;
    return yield* updateArchiveActionEffect(
      action.actionId,
      "queued",
      terminal ? "failed" : "queued",
      {
        reason,
        attempts,
        nextAttemptAt: now + Math.min(30 * 60_000, 60_000 * 2 ** attempts),
      },
    );
  }
  const snapshot = inspected.success;
  if (snapshot.strategy === "uidplus_copy") {
    if (!copyTransport(transport) || !snapshot.targetFolder)
      return yield* updateArchiveActionEffect(action.actionId, "queued", "failed", {
        reason: "transport_unavailable",
      });
    const claimed = yield* updateArchiveActionEffect(
      action.actionId,
      "queued",
      "copy_claimed",
      { snapshot, attempts: action.attempts + 1, reason: "copy_uncertain" },
    );
    const copy = yield* copyTransport(transport)!
      .copy(source, snapshot.targetFolder, snapshot)
      .pipe(Effect.result);
    return yield* advanceCopyActionEffect(
      claimed,
      transport,
      false,
      copy._tag === "Success" ? copy.success : undefined,
    );
  }
  const claimed = yield* updateArchiveActionEffect(
    action.actionId,
    "queued",
    "claimed",
    { snapshot, reason: "uncertain", attempts: action.attempts + 1 },
  );
  // From this point a crash may have lost the MOVE response. Never retry MOVE.
  const moved = yield* api.move(source, snapshot.sourceHash).pipe(Effect.result);
  if (moved._tag === "Success") {
    const verified = yield* api
      .verify(
        moved.success.destination,
        action.identity.messageId,
        snapshot.sourceHash,
        snapshot.flags,
      )
      .pipe(Effect.result);
    if (verified._tag === "Success" && verified.success) {
      const confirmed = yield* api
        .reconcile(source, snapshot.sourceHash)
        .pipe(Effect.result);
      if (
        confirmed._tag === "Success" &&
        confirmed.success.state === "moved" &&
        confirmed.success.destination.folder === moved.success.destination.folder &&
        confirmed.success.destination.uidValidity ===
          moved.success.destination.uidValidity &&
        confirmed.success.destination.uid === moved.success.destination.uid &&
        JSON.stringify(confirmed.success.snapshot.flags) ===
          JSON.stringify(snapshot.flags)
      )
        return yield* updateArchiveActionEffect(
          action.actionId,
          "claimed",
          "archived",
          { destination: moved.success.destination, reason: undefined },
        );
    }
  }
  return yield* reconcileClaimedArchiveEffect(claimed, transport);
});

export const reconcileClaimedArchiveEffect = Effect.fn("EmailArchive.reconcile")(
  function* (action: ArchiveAction, transport: Transport) {
    if (
      (action.status !== "claimed" && action.status !== "uncertain") ||
      !action.snapshot
    )
      return action;
    const api = archiveTransport(transport);
    if (!api)
      return yield* new ArchiveActionError({
        message: "Active email transport does not support archive actions",
      });
    const result = yield* api
      .reconcile(yield* archiveSourceEffect(action), action.snapshot.sourceHash)
      .pipe(Effect.result);
    if (result._tag === "Failure") {
      if (action.status === "claimed")
        return yield* updateArchiveActionEffect(
          action.actionId,
          "claimed",
          "uncertain",
        );
      return action;
    }
    const checked = result.success;
    if (
      checked.state === "moved" &&
      JSON.stringify(checked.snapshot.flags) === JSON.stringify(action.snapshot.flags)
    )
      return yield* updateArchiveActionEffect(
        action.actionId,
        action.status,
        "archived",
        { destination: checked.destination, reason: undefined },
      );
    if (checked.state === "not_moved")
      return yield* updateArchiveActionEffect(
        action.actionId,
        action.status,
        "failed",
        { reason: "verification_failed" },
      );
    if (action.status === "claimed")
      return yield* updateArchiveActionEffect(action.actionId, "claimed", "uncertain");
    return action;
  },
);

const restoreArchivedActionUnlockedEffect = Effect.fn("EmailArchive.restore")(
  function* (actionId: string, transport: Transport) {
    const action = yield* getArchiveActionEffect(actionId);
    if (!action)
      return yield* new ArchiveActionError({ message: "Archive action not found" });
    if (action.status === "restored") return action;
    if (action.status !== "archived" || !action.snapshot || !action.destination)
      return yield* new ArchiveActionError({
        message: `Archive action cannot be restored from ${action.status}`,
      });
    // Recovery matches by Message-ID and content, so one copy moves at a time.
    const siblings = yield* listArchiveActionsForMessageEffect(
      action.identity.messageId,
    );
    if (
      siblings.some(
        (sibling) =>
          sibling.actionId !== actionId && !isSettledArchiveStatus(sibling.status),
      )
    )
      return yield* new ArchiveActionError({
        message: "Another copy with this Message-ID has an unresolved archive action",
      });
    const api = archiveTransport(transport);
    if (!api)
      return yield* new ArchiveActionError({
        message: "Active email transport does not support archive actions",
      });
    const inspected = yield* api.inspectDestination(
      action.identity,
      action.destination,
    );
    if (inspected.sourceHash !== action.snapshot.sourceHash)
      return yield* new ArchiveActionError({
        message: "Archived message content changed; restore refused",
      });
    if (
      action.snapshot.strategy === "uidplus_copy" ||
      inspected.strategy === "uidplus_copy"
    ) {
      if (!copyTransport(transport))
        return yield* new ArchiveActionError({
          message: "UIDPLUS archive transport unavailable",
        });
      const restoreSnapshot = {
        ...inspected,
        strategy: "uidplus_copy" as const,
        targetFolder: "INBOX",
      };
      const claimed = yield* updateArchiveActionEffect(
        actionId,
        "archived",
        "restore_copy_claimed",
        { restoreSnapshot, reason: "copy_uncertain" },
      );
      const source = { ...action.destination, messageId: action.identity.messageId };
      const copy = yield* copyTransport(transport)!
        .copy(source, "INBOX", restoreSnapshot)
        .pipe(Effect.result);
      return yield* advanceCopyActionEffect(
        claimed,
        transport,
        true,
        copy._tag === "Success" ? copy.success : undefined,
      );
    }
    const claimed = yield* updateArchiveActionEffect(
      actionId,
      "archived",
      "restore_claimed",
      { restoreSnapshot: inspected, reason: "uncertain" },
    );
    const moved = yield* api
      .restore(action.identity, action.destination, action.snapshot.sourceHash)
      .pipe(Effect.result);
    if (moved._tag === "Success") {
      const verified = yield* api
        .verify(
          moved.success.destination,
          action.identity.messageId,
          action.snapshot.sourceHash,
          inspected.flags,
        )
        .pipe(Effect.result);
      if (verified._tag === "Success" && verified.success) {
        const confirmed = yield* api
          .reconcileRestore(
            action.identity,
            action.destination,
            action.snapshot.sourceHash,
          )
          .pipe(Effect.result);
        if (
          confirmed._tag === "Success" &&
          confirmed.success.state === "moved" &&
          confirmed.success.destination.folder === moved.success.destination.folder &&
          confirmed.success.destination.uidValidity ===
            moved.success.destination.uidValidity &&
          confirmed.success.destination.uid === moved.success.destination.uid &&
          JSON.stringify(confirmed.success.snapshot.flags) ===
            JSON.stringify(inspected.flags)
        )
          return yield* updateArchiveActionEffect(
            actionId,
            "restore_claimed",
            "restored",
            { restoredLocation: moved.success.destination, reason: undefined },
          );
      }
    }
    return yield* reconcileClaimedRestoreEffect(claimed, transport);
  },
);

export const restoreArchivedActionEffect = (actionId: string, transport: Transport) =>
  withArchiveActionWorkflowEffect(
    restoreArchivedActionUnlockedEffect(actionId, transport),
  );

export const reconcileClaimedRestoreEffect = Effect.fn("EmailArchive.reconcileRestore")(
  function* (action: ArchiveAction, transport: Transport) {
    if (
      (action.status !== "restore_claimed" && action.status !== "restore_uncertain") ||
      !action.snapshot ||
      !action.restoreSnapshot ||
      !action.destination
    )
      return action;
    const api = archiveTransport(transport);
    if (!api)
      return yield* new ArchiveActionError({
        message: "Active email transport does not support archive actions",
      });
    const result = yield* api
      .reconcileRestore(action.identity, action.destination, action.snapshot.sourceHash)
      .pipe(Effect.result);
    if (
      result._tag === "Success" &&
      result.success.state === "moved" &&
      JSON.stringify(result.success.snapshot.flags) ===
        JSON.stringify(action.restoreSnapshot.flags)
    )
      return yield* updateArchiveActionEffect(
        action.actionId,
        action.status,
        "restored",
        { restoredLocation: result.success.destination, reason: undefined },
      );
    if (action.status === "restore_claimed")
      return yield* updateArchiveActionEffect(
        action.actionId,
        "restore_claimed",
        "restore_uncertain",
      );
    return action;
  },
);

/** One durable step at a time. A claimed COPY/STORE/EXPUNGE is never repeated. */
export const advanceCopyActionEffect = Effect.fn("EmailArchive.advanceCopy")(function* (
  action: ArchiveAction,
  transport: Transport,
  reverse: boolean,
  mappedDestination?: ArchiveLocation,
  allowMutation = true,
) {
  const api = copyTransport(transport);
  const snapshot = reverse ? action.restoreSnapshot : action.snapshot;
  const source = reverse
    ? action.destination && {
        ...action.destination,
        messageId: action.identity.messageId,
      }
    : yield* archiveSourceEffect(action);
  const target = reverse ? "INBOX" : snapshot?.targetFolder;
  if (!api || !snapshot || !source || !target)
    return yield* new ArchiveActionError({
      message: "Incomplete UIDPLUS archive receipt",
    });
  const copyClaimed = reverse ? "restore_copy_claimed" : "copy_claimed";
  const copyVerified = reverse ? "restore_copy_verified" : "copy_verified";
  const deleteClaimed = reverse ? "restore_delete_claimed" : "delete_claimed";
  const expungeClaimed = reverse ? "restore_expunge_claimed" : "expunge_claimed";
  const retained = reverse
    ? "restore_copied_source_retained"
    : "copied_source_retained";
  const completed = reverse ? "restored" : "archived";
  const locationPatch = (value: ArchiveLocation) =>
    reverse ? { restoredLocation: value } : { destination: value };
  let current = action;

  if (current.status === copyClaimed) {
    let expectedLocation = reverse ? current.restoredLocation : current.destination;
    const sameLocation = (left: ArchiveLocation, right: ArchiveLocation) =>
      left.folder === right.folder &&
      left.uidValidity === right.uidValidity &&
      left.uid === right.uid;
    if (
      mappedDestination &&
      expectedLocation &&
      !sameLocation(mappedDestination, expectedLocation)
    )
      return current;
    if (mappedDestination && !expectedLocation) {
      // The COPYUID response is stronger than a later Message-ID search. Store it
      // before reconciliation so a restart cannot accept a different UID.
      current = yield* updateArchiveActionEffect(
        action.actionId,
        copyClaimed,
        copyClaimed,
        locationPatch(mappedDestination),
      );
      expectedLocation = mappedDestination;
    }
    const read = yield* api.reconcileCopy(source, target, snapshot).pipe(Effect.result);
    if (
      read._tag === "Failure" ||
      read.success.state !== "moved" ||
      (expectedLocation && !sameLocation(expectedLocation, read.success.destination))
    )
      return current;
    current = yield* updateArchiveActionEffect(
      action.actionId,
      copyClaimed,
      copyVerified,
      { ...locationPatch(read.success.destination), reason: undefined },
    );
  }
  if (!allowMutation && current.status === copyVerified) return current;
  const copiedLocation = reverse ? current.restoredLocation : current.destination;
  if (!copiedLocation) return current;

  if (current.status === copyVerified) {
    current = yield* updateArchiveActionEffect(
      action.actionId,
      copyVerified,
      deleteClaimed,
      { reason: "source_mark_uncertain" },
    );
    yield* api.mark(source, copiedLocation, snapshot).pipe(Effect.result);
  }
  if (current.status === deleteClaimed) {
    const read = yield* api
      .inspect(source, copiedLocation, snapshot)
      .pipe(Effect.result);
    if (read._tag === "Failure" || read.success === "uncertain") return current;
    if (read.success === "unmarked")
      return yield* updateArchiveActionEffect(
        action.actionId,
        deleteClaimed,
        retained,
        { reason: "copied_source_retained" },
      );
    if (read.success === "absent")
      return yield* updateArchiveActionEffect(
        action.actionId,
        deleteClaimed,
        completed,
        { reason: undefined },
      );
    if (!allowMutation) return current;
    current = yield* updateArchiveActionEffect(
      action.actionId,
      deleteClaimed,
      expungeClaimed,
      { reason: "source_expunge_uncertain" },
    );
    yield* api.expunge(source, copiedLocation, snapshot).pipe(Effect.result);
  }
  if (current.status === expungeClaimed) {
    const read = yield* api
      .inspect(source, copiedLocation, snapshot)
      .pipe(Effect.result);
    if (read._tag === "Failure" || read.success === "uncertain") return current;
    if (read.success === "absent")
      return yield* updateArchiveActionEffect(
        action.actionId,
        expungeClaimed,
        completed,
        { reason: undefined },
      );
    return yield* updateArchiveActionEffect(action.actionId, expungeClaimed, retained, {
      reason:
        read.success === "marked" ? "copied_source_deleted" : "copied_source_retained",
    });
  }
  if (current.status === retained) {
    const read = yield* api
      .inspect(source, copiedLocation, snapshot)
      .pipe(Effect.result);
    const nextAttemptAt = (yield* Clock.currentTimeMillis) + 5 * 60_000;
    if (read._tag === "Failure" || read.success === "uncertain")
      return yield* updateArchiveActionEffect(action.actionId, retained, retained, {
        nextAttemptAt,
      });
    if (read.success === "absent")
      return yield* updateArchiveActionEffect(action.actionId, retained, completed, {
        reason: undefined,
      });
    const reason =
      read.success === "marked"
        ? ("copied_source_deleted" as const)
        : ("copied_source_retained" as const);
    return yield* updateArchiveActionEffect(action.actionId, retained, retained, {
      reason,
      nextAttemptAt,
    });
  }
  return current;
});

/** Bounded sweep. It reconciles claimed operations without repeating them, then
 * may advance a verified action to its next durably claimed operation. */
export const processQueuedArchiveActionsEffect = Effect.fn("EmailArchive.sweep")(
  function* (transport: Transport) {
    const now = yield* Clock.currentTimeMillis;
    const due = (yield* listArchiveActionsEffect())
      .filter((action) =>
        [
          "queued",
          "claimed",
          "uncertain",
          "restore_claimed",
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
        ].includes(action.status),
      )
      .filter((action) => action.nextAttemptAt <= now);
    const byDue = (a: ArchiveAction, b: ArchiveAction) =>
      a.nextAttemptAt - b.nextAttemptAt || a.createdAt - b.createdAt;
    const actionable = due
      .filter((action) =>
        ["queued", "copy_verified", "restore_copy_verified"].includes(action.status),
      )
      .sort(byDue);
    const recovery = due
      .filter(
        (action) =>
          !["queued", "copy_verified", "restore_copy_verified"].includes(action.status),
      )
      .sort(byDue);
    const actions = [
      ...actionable.slice(0, 10),
      ...recovery.slice(0, 10),
      ...[...actionable.slice(10), ...recovery.slice(10)]
        .sort(byDue)
        .slice(0, 20 - Math.min(10, actionable.length) - Math.min(10, recovery.length)),
    ];
    for (const scheduled of actions) {
      yield* withArchiveActionWorkflowEffect(
        Effect.gen(function* () {
          const action = yield* getArchiveActionEffect(scheduled.actionId);
          if (!action || action.status !== scheduled.status) return;
          const process =
            action.status === "queued"
              ? processArchiveActionEffect(action, transport)
              : action.status === "claimed" || action.status === "uncertain"
                ? reconcileClaimedArchiveEffect(action, transport)
                : action.status.startsWith("restore_copy_") ||
                    action.status === "restore_delete_claimed" ||
                    action.status === "restore_expunge_claimed" ||
                    action.status === "restore_copied_source_retained"
                  ? advanceCopyActionEffect(action, transport, true)
                  : action.status === "copy_claimed" ||
                      action.status === "copy_verified" ||
                      action.status === "delete_claimed" ||
                      action.status === "expunge_claimed" ||
                      action.status === "copied_source_retained"
                    ? advanceCopyActionEffect(action, transport, false)
                    : reconcileClaimedRestoreEffect(action, transport);
          const result = yield* process.pipe(Effect.result);
          if (
            action.status !== "queued" &&
            !action.status.endsWith("copied_source_retained") &&
            (result._tag === "Failure" || result.success.status === action.status)
          )
            yield* updateArchiveActionEffect(
              action.actionId,
              action.status,
              action.status,
              { nextAttemptAt: now + 5 * 60_000 },
            ).pipe(Effect.ignore);
        }),
      );
    }
  },
);
