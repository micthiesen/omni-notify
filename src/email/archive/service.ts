import { Clock, Effect } from "effect";
import type { TaskServices } from "../../task-runs/registry.js";
import type { EmailTransport } from "../types.js";
import { ArchiveActionError, type ArchiveAction } from "./persistence.js";
import {
  getArchiveActionEffect,
  listArchiveActionsEffect,
  updateArchiveActionEffect,
} from "./persistence.js";

type Transport = EmailTransport<unknown, TaskServices>;

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

function inspectFailureReason(cause: unknown): ArchiveAction["reason"] {
  const message = cause instanceof Error ? cause.message : "";
  if (message.includes("does not advertise MOVE")) return "native_move_unavailable";
  if (
    message.includes("Inbox UID is gone") ||
    message.includes("UIDVALIDITY changed") ||
    message.includes("different Message-ID")
  )
    return "source_unavailable";
  return "transport_unavailable";
}

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
  const inspected = yield* api.inspect(action.identity).pipe(Effect.result);
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
  const claimed = yield* updateArchiveActionEffect(
    action.actionId,
    "queued",
    "claimed",
    { snapshot, reason: "uncertain", attempts: action.attempts + 1 },
  );
  // From this point a crash may have lost the MOVE response. Never retry MOVE.
  const moved = yield* api
    .move(action.identity, snapshot.sourceHash)
    .pipe(Effect.result);
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
        .reconcile(action.identity, snapshot.sourceHash)
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
      .reconcile(action.identity, action.snapshot.sourceHash)
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

export const restoreArchivedActionEffect = Effect.fn("EmailArchive.restore")(function* (
  actionId: string,
  transport: Transport,
) {
  const action = yield* getArchiveActionEffect(actionId);
  if (!action)
    return yield* new ArchiveActionError({ message: "Archive action not found" });
  if (action.status === "restored") return action;
  if (action.status !== "archived" || !action.snapshot || !action.destination)
    return yield* new ArchiveActionError({
      message: `Archive action cannot be restored from ${action.status}`,
    });
  const api = archiveTransport(transport);
  if (!api)
    return yield* new ArchiveActionError({
      message: "Active email transport does not support archive actions",
    });
  const inspected = yield* api.inspectDestination(action.identity, action.destination);
  if (inspected.sourceHash !== action.snapshot.sourceHash)
    return yield* new ArchiveActionError({
      message: "Archived message content changed; restore refused",
    });
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
});

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

/** Bounded boot/periodic sweep. Claimed actions only get read-only recovery. */
export const processQueuedArchiveActionsEffect = Effect.fn("EmailArchive.sweep")(
  function* (transport: Transport) {
    const now = yield* Clock.currentTimeMillis;
    const actions = (yield* listArchiveActionsEffect())
      .filter((action) =>
        [
          "queued",
          "claimed",
          "uncertain",
          "restore_claimed",
          "restore_uncertain",
        ].includes(action.status),
      )
      .filter((action) => action.status !== "queued" || action.nextAttemptAt <= now)
      .sort((a, b) => a.createdAt - b.createdAt)
      .slice(0, 20);
    for (const action of actions) {
      if (action.status === "queued")
        yield* processArchiveActionEffect(action, transport).pipe(Effect.ignore);
      else if (action.status === "claimed" || action.status === "uncertain")
        yield* reconcileClaimedArchiveEffect(action, transport).pipe(Effect.ignore);
      else yield* reconcileClaimedRestoreEffect(action, transport).pipe(Effect.ignore);
    }
  },
);
