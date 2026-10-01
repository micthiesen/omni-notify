import { createHash } from "node:crypto";
import { Docstore } from "@micthiesen/mitools/docstore";
import { Deferred, Effect, Fiber } from "effect";
import { afterAll, describe, expect, it, vi } from "vitest";
import type { TaskServices } from "../../task-runs/registry.js";
import { createMitoolsTestRuntime } from "../../test/mitools.js";
import type { EmailTransport } from "../types.js";
import {
  archiveAutoReadProtectionEffect,
  getArchiveActionEffect,
  isArchiveActionMessageEffect,
  queueArchiveActionEffect,
  updateArchiveActionEffect,
} from "./persistence.js";
import {
  advanceCopyActionEffect,
  processArchiveActionEffect,
  processQueuedArchiveActionsEffect,
  restoreArchivedActionEffect,
  withArchiveActionWorkflowEffect,
} from "./service.js";

const testRuntime = createMitoolsTestRuntime();
afterAll(() => testRuntime.dispose());
const run = testRuntime.run;

function identity(id: string) {
  return {
    folder: "INBOX",
    uidValidity: "10",
    uid: 7,
    messageId: `<${id}@example.test>`,
  };
}

function transport(overrides: Record<string, unknown> = {}) {
  const methods = {
    inspectArchiveMessageEffect: vi.fn(() =>
      Effect.succeed({ sourceHash: "abc", flags: ["\\Flagged"] }),
    ),
    moveArchiveMessageEffect: vi.fn(() =>
      Effect.succeed({
        sourceHash: "abc",
        flags: ["\\Flagged"],
        destination: { folder: "Archive", uidValidity: "20", uid: 12 },
      }),
    ),
    reconcileArchiveMessageEffect: vi.fn(() =>
      Effect.succeed({
        state: "moved",
        destination: { folder: "Archive", uidValidity: "20", uid: 12 },
        snapshot: { sourceHash: "abc", flags: ["\\Flagged"] },
      }),
    ),
    inspectArchiveDestinationEffect: vi.fn(() =>
      Effect.succeed({ sourceHash: "abc", flags: ["\\Seen"] }),
    ),
    restoreArchiveMessageEffect: vi.fn(() =>
      Effect.succeed({
        sourceHash: "abc",
        flags: ["\\Seen"],
        destination: { folder: "INBOX", uidValidity: "10", uid: 8 },
      }),
    ),
    reconcileRestoreMessageEffect: vi.fn(() =>
      Effect.succeed({
        state: "moved",
        destination: { folder: "INBOX", uidValidity: "10", uid: 8 },
        snapshot: { sourceHash: "abc", flags: ["\\Seen"] },
      }),
    ),
    verifyArchiveLocationEffect: vi.fn(() => Effect.succeed(true)),
    ...overrides,
  };
  return methods as typeof methods & EmailTransport<unknown, TaskServices>;
}

describe("durable email archive actions", () => {
  it("suppresses UIDPLUS echoes only at recorded coordinates", async () => {
    const source = identity("copy-echo");
    const queued = await run(queueArchiveActionEffect("copy-echo", source));
    const archive = { folder: "Archive", uidValidity: "20", uid: 12 };
    const unrelated = { ...archive, uid: 13 };
    const claimed = await run(
      updateArchiveActionEffect(queued.actionId, "queued", "copy_claimed", {
        snapshot: {
          sourceHash: "abc",
          flags: [],
          strategy: "uidplus_copy",
          targetFolder: "Archive",
        },
      }),
    );
    expect(claimed.status).toBe("copy_claimed");
    expect(await run(isArchiveActionMessageEffect(source.messageId, archive))).toBe(
      false,
    );
    const verified = await run(
      updateArchiveActionEffect(queued.actionId, "copy_claimed", "copy_verified", {
        destination: archive,
      }),
    );
    expect(verified.status).toBe("copy_verified");
    expect(await run(isArchiveActionMessageEffect(source.messageId, archive))).toBe(
      true,
    );
    expect(await run(isArchiveActionMessageEffect(source.messageId, unrelated))).toBe(
      false,
    );
    expect(await run(isArchiveActionMessageEffect(source.messageId, source))).toBe(
      false,
    );
    await run(updateArchiveActionEffect(queued.actionId, "copy_verified", "failed"));
  });

  it("rechecks a retained copy after delayed STORE or EXPUNGE without mutating", async () => {
    const queued = await run(
      queueArchiveActionEffect("copy-delayed", identity("copy-delayed")),
    );
    const destination = { folder: "Archive", uidValidity: "20", uid: 12 };
    const snapshot = {
      sourceHash: "abc",
      flags: [],
      strategy: "uidplus_copy" as const,
      targetFolder: "Archive",
    };
    const retained = await run(
      updateArchiveActionEffect(queued.actionId, "queued", "copied_source_retained", {
        snapshot,
        destination,
        reason: "copied_source_retained",
      }),
    );
    let observed: "unmarked" | "marked" | "absent" = "unmarked";
    const methods = transport({
      copyExactArchiveMessageEffect: vi.fn(() => Effect.succeed(destination)),
      reconcileExactCopyEffect: vi.fn(() =>
        Effect.succeed({ state: "uncertain" as const }),
      ),
      markExactArchiveSourceDeletedEffect: vi.fn(() => Effect.succeed(true)),
      inspectExactDeletedSourceEffect: vi.fn(() => Effect.sync(() => observed)),
      expungeExactArchiveSourceEffect: vi.fn(() => Effect.succeed(true)),
    });
    expect(
      (await run(advanceCopyActionEffect(retained, methods, false, undefined, false)))
        .status,
    ).toBe("copied_source_retained");
    observed = "marked";
    await run(
      updateArchiveActionEffect(
        queued.actionId,
        "copied_source_retained",
        "copied_source_retained",
        { nextAttemptAt: 0 },
      ),
    );
    await run(processQueuedArchiveActionsEffect(methods));
    expect((await run(getArchiveActionEffect(queued.actionId)))?.reason).toBe(
      "copied_source_deleted",
    );
    observed = "absent";
    const latest = (await run(getArchiveActionEffect(queued.actionId)))!;
    expect(
      (await run(advanceCopyActionEffect(latest, methods, false, undefined, false)))
        .status,
    ).toBe("archived");
    expect(
      vi
        .mocked(methods.copyExactArchiveMessageEffect!)
        .mock.calls.filter(
          ([source]) => source.messageId === queued.identity.messageId,
        ),
    ).toHaveLength(0);
    expect(
      vi
        .mocked(methods.markExactArchiveSourceDeletedEffect!)
        .mock.calls.filter(
          ([source]) => source.messageId === queued.identity.messageId,
        ),
    ).toHaveLength(0);
    expect(
      vi
        .mocked(methods.expungeExactArchiveSourceEffect!)
        .mock.calls.filter(
          ([source]) => source.messageId === queued.identity.messageId,
        ),
    ).toHaveLength(0);
  });

  it("rechecks a retained restore copy without deleting Archive again", async () => {
    const queued = await run(
      queueArchiveActionEffect("restore-delayed", identity("restore-delayed")),
    );
    const destination = { folder: "Archive", uidValidity: "20", uid: 12 };
    const restoredLocation = { folder: "INBOX", uidValidity: "10", uid: 8 };
    const retained = await run(
      updateArchiveActionEffect(
        queued.actionId,
        "queued",
        "restore_copied_source_retained",
        {
          snapshot: {
            sourceHash: "abc",
            flags: [],
            strategy: "uidplus_copy",
            targetFolder: "Archive",
          },
          restoreSnapshot: {
            sourceHash: "abc",
            flags: ["\\Seen"],
            strategy: "uidplus_copy",
            targetFolder: "INBOX",
          },
          destination,
          restoredLocation,
          reason: "copied_source_deleted",
        },
      ),
    );
    const methods = transport({
      copyExactArchiveMessageEffect: vi.fn(() => Effect.succeed(restoredLocation)),
      reconcileExactCopyEffect: vi.fn(() =>
        Effect.succeed({ state: "uncertain" as const }),
      ),
      markExactArchiveSourceDeletedEffect: vi.fn(() => Effect.succeed(true)),
      inspectExactDeletedSourceEffect: vi.fn(() => Effect.succeed("absent" as const)),
      expungeExactArchiveSourceEffect: vi.fn(() => Effect.succeed(true)),
    });
    expect(
      (await run(advanceCopyActionEffect(retained, methods, true, undefined, false)))
        .status,
    ).toBe("restored");
    expect(methods.markExactArchiveSourceDeletedEffect).not.toHaveBeenCalled();
    expect(methods.expungeExactArchiveSourceEffect).not.toHaveBeenCalled();
  });

  it("does not let retained receipts starve a newer queued action", async () => {
    const destination = { folder: "Archive", uidValidity: "20", uid: 12 };
    for (let index = 0; index < 21; index += 1) {
      const queued = await run(
        queueArchiveActionEffect(
          `retained-priority-${index}`,
          identity(`retained-priority-${index}`),
        ),
      );
      await run(
        updateArchiveActionEffect(queued.actionId, "queued", "copied_source_retained", {
          snapshot: {
            sourceHash: "abc",
            flags: [],
            strategy: "uidplus_copy",
            targetFolder: "Archive",
          },
          destination,
          reason: "copied_source_retained",
        }),
      );
    }
    const queued = await run(
      queueArchiveActionEffect("priority-fresh", identity("priority-fresh")),
    );
    const methods = transport({
      copyExactArchiveMessageEffect: vi.fn(() => Effect.succeed(destination)),
      reconcileExactCopyEffect: vi.fn(() =>
        Effect.succeed({ state: "uncertain" as const }),
      ),
      markExactArchiveSourceDeletedEffect: vi.fn(() => Effect.succeed(true)),
      inspectExactDeletedSourceEffect: vi.fn(() => Effect.succeed("unmarked" as const)),
      expungeExactArchiveSourceEffect: vi.fn(() => Effect.succeed(true)),
    });
    await run(processQueuedArchiveActionsEffect(methods));
    expect((await run(getArchiveActionEffect(queued.actionId)))?.status).toBe(
      "archived",
    );
    expect(methods.moveArchiveMessageEffect).toHaveBeenCalledOnce();
  });

  it("does not let unresolved COPY claims starve a newer queued action", async () => {
    for (let index = 0; index < 20; index += 1) {
      const queued = await run(
        queueArchiveActionEffect(
          `uncertain-priority-${index}`,
          identity(`uncertain-priority-${index}`),
        ),
      );
      await run(
        updateArchiveActionEffect(queued.actionId, "queued", "copy_claimed", {
          snapshot: {
            sourceHash: "abc",
            flags: [],
            strategy: "uidplus_copy",
            targetFolder: "Archive",
          },
          reason: "copy_uncertain",
        }),
      );
    }
    const queued = await run(
      queueArchiveActionEffect(
        "priority-after-claims",
        identity("priority-after-claims"),
      ),
    );
    const methods = transport({
      copyExactArchiveMessageEffect: vi.fn(() =>
        Effect.succeed({ folder: "Archive", uidValidity: "20", uid: 12 }),
      ),
      reconcileExactCopyEffect: vi.fn(() =>
        Effect.succeed({ state: "uncertain" as const }),
      ),
      markExactArchiveSourceDeletedEffect: vi.fn(() => Effect.succeed(true)),
      inspectExactDeletedSourceEffect: vi.fn(() =>
        Effect.succeed("uncertain" as const),
      ),
      expungeExactArchiveSourceEffect: vi.fn(() => Effect.succeed(true)),
    });
    await run(processQueuedArchiveActionsEffect(methods));
    expect((await run(getArchiveActionEffect(queued.actionId)))?.status).toBe(
      "archived",
    );
    expect(methods.moveArchiveMessageEffect).toHaveBeenCalledOnce();
  });
  it("persists COPY, Deleted, and UID EXPUNGE claims before each mutation", async () => {
    const queued = await run(
      queueArchiveActionEffect("copy-stages", identity("copy-stages")),
    );
    const destination = { folder: "Archive", uidValidity: "20", uid: 12 };
    let sourceState: "present" | "marked" | "absent" = "present";
    const methods = transport({
      inspectArchiveMessageEffect: vi.fn(() =>
        Effect.succeed({
          sourceHash: "abc",
          flags: ["\\Flagged"],
          strategy: "uidplus_copy",
          targetFolder: "Archive",
        }),
      ),
      copyExactArchiveMessageEffect: vi.fn(() =>
        Effect.gen(function* () {
          expect((yield* getArchiveActionEffect(queued.actionId))?.status).toBe(
            "copy_claimed",
          );
          return destination;
        }),
      ),
      reconcileExactCopyEffect: vi.fn(() =>
        Effect.succeed({
          state: "moved",
          destination,
          snapshot: { sourceHash: "abc", flags: ["\\Flagged"] },
        }),
      ),
      markExactArchiveSourceDeletedEffect: vi.fn(() =>
        Effect.gen(function* () {
          expect((yield* getArchiveActionEffect(queued.actionId))?.status).toBe(
            "delete_claimed",
          );
          sourceState = "marked";
          return true;
        }),
      ),
      inspectExactDeletedSourceEffect: vi.fn(() => Effect.sync(() => sourceState)),
      expungeExactArchiveSourceEffect: vi.fn(() =>
        Effect.gen(function* () {
          expect((yield* getArchiveActionEffect(queued.actionId))?.status).toBe(
            "expunge_claimed",
          );
          sourceState = "absent";
          return true;
        }),
      ),
    });
    const result = await run(processArchiveActionEffect(queued, methods));
    expect(result.status).toBe("archived");
    expect(result.destination).toEqual(destination);
    expect(methods.copyExactArchiveMessageEffect).toHaveBeenCalledOnce();
    expect(methods.markExactArchiveSourceDeletedEffect).toHaveBeenCalledOnce();
    expect(methods.expungeExactArchiveSourceEffect).toHaveBeenCalledOnce();
  });

  it("never recopies after a lost COPY response and keeps status reconciliation read-only", async () => {
    const queued = await run(
      queueArchiveActionEffect("copy-lost", identity("copy-lost")),
    );
    const destination = { folder: "Archive", uidValidity: "20", uid: 12 };
    let copied = false;
    const methods = transport({
      inspectArchiveMessageEffect: vi.fn(() =>
        Effect.succeed({
          sourceHash: "abc",
          flags: [],
          strategy: "uidplus_copy",
          targetFolder: "Archive",
        }),
      ),
      copyExactArchiveMessageEffect: vi.fn(() =>
        Effect.fail(new Error("response lost")),
      ),
      reconcileExactCopyEffect: vi.fn(() =>
        Effect.sync(() =>
          copied
            ? {
                state: "moved" as const,
                destination,
                snapshot: { sourceHash: "abc", flags: [] },
              }
            : { state: "uncertain" as const },
        ),
      ),
      markExactArchiveSourceDeletedEffect: vi.fn(() => Effect.succeed(true)),
      inspectExactDeletedSourceEffect: vi.fn(() => Effect.succeed("unmarked" as const)),
      expungeExactArchiveSourceEffect: vi.fn(() => Effect.succeed(true)),
    });
    const first = await run(processArchiveActionEffect(queued, methods));
    expect(first.status).toBe("copy_claimed");
    copied = true;
    const reconciled = await run(
      advanceCopyActionEffect(first, methods, false, undefined, false),
    );
    expect(reconciled.status).toBe("copy_verified");
    expect(methods.markExactArchiveSourceDeletedEffect).not.toHaveBeenCalled();
    await run(processQueuedArchiveActionsEffect(methods));
    expect(methods.copyExactArchiveMessageEffect).toHaveBeenCalledOnce();
    expect(
      vi
        .mocked(methods.markExactArchiveSourceDeletedEffect!)
        .mock.calls.filter(
          ([source]) => source.messageId === queued.identity.messageId,
        ),
    ).toHaveLength(1);
    expect(
      vi
        .mocked(methods.expungeExactArchiveSourceEffect!)
        .mock.calls.filter(
          ([source]) => source.messageId === queued.identity.messageId,
        ),
    ).toHaveLength(0);
  });

  it("persists COPYUID before reconciliation and rejects a different UID after restart", async () => {
    const queued = await run(
      queueArchiveActionEffect("copyuid-restart", identity("copyuid-restart")),
    );
    const claimed = await run(
      updateArchiveActionEffect(queued.actionId, "queued", "copy_claimed", {
        snapshot: {
          sourceHash: "abc",
          flags: [],
          strategy: "uidplus_copy",
          targetFolder: "Archive",
        },
      }),
    );
    const mapped = { folder: "Archive", uidValidity: "20", uid: 12 };
    const different = { ...mapped, uid: 13 };
    const methods = transport({
      copyExactArchiveMessageEffect: vi.fn(() => Effect.succeed(mapped)),
      reconcileExactCopyEffect: vi.fn(() =>
        Effect.succeed({
          state: "moved" as const,
          destination: different,
          snapshot: { sourceHash: "abc", flags: [] },
        }),
      ),
      markExactArchiveSourceDeletedEffect: vi.fn(() => Effect.succeed(true)),
      inspectExactDeletedSourceEffect: vi.fn(() => Effect.succeed("unmarked" as const)),
      expungeExactArchiveSourceEffect: vi.fn(() => Effect.succeed(true)),
    });
    const first = await run(advanceCopyActionEffect(claimed, methods, false, mapped));
    expect(first.status).toBe("copy_claimed");
    expect(first.destination).toEqual(mapped);
    const restarted = (await run(getArchiveActionEffect(queued.actionId)))!;
    const second = await run(advanceCopyActionEffect(restarted, methods, false));
    expect(second.status).toBe("copy_claimed");
    expect(second.destination).toEqual(mapped);
    expect(methods.copyExactArchiveMessageEffect).not.toHaveBeenCalled();
    expect(methods.markExactArchiveSourceDeletedEffect).not.toHaveBeenCalled();
    expect(methods.expungeExactArchiveSourceEffect).not.toHaveBeenCalled();
  });

  it("holds status reconciliation until an in-flight COPY records its UID", async () => {
    const queued = await run(
      queueArchiveActionEffect(
        "copy-status-concurrent",
        identity("copy-status-concurrent"),
      ),
    );
    const destination = { folder: "Archive", uidValidity: "20", uid: 12 };
    await run(
      Effect.gen(function* () {
        const copyStarted = yield* Deferred.make<void>();
        const releaseCopy = yield* Deferred.make<void>();
        const statusStarted = yield* Deferred.make<void>();
        let sourceState: "marked" | "absent" = "marked";
        const methods = transport({
          inspectArchiveMessageEffect: vi.fn(() =>
            Effect.succeed({
              sourceHash: "abc",
              flags: [],
              strategy: "uidplus_copy",
              targetFolder: "Archive",
            }),
          ),
          copyExactArchiveMessageEffect: vi.fn(() =>
            Effect.gen(function* () {
              yield* Deferred.succeed(copyStarted, undefined);
              yield* Deferred.await(releaseCopy);
              return destination;
            }),
          ),
          reconcileExactCopyEffect: vi.fn(() =>
            Effect.succeed({
              state: "moved" as const,
              destination,
              snapshot: { sourceHash: "abc", flags: [] },
            }),
          ),
          markExactArchiveSourceDeletedEffect: vi.fn(() => Effect.succeed(true)),
          inspectExactDeletedSourceEffect: vi.fn(() => Effect.sync(() => sourceState)),
          expungeExactArchiveSourceEffect: vi.fn(() =>
            Effect.sync(() => {
              sourceState = "absent";
              return true;
            }),
          ),
        });
        const worker = yield* Effect.forkChild(
          processQueuedArchiveActionsEffect(methods),
        );
        yield* Deferred.await(copyStarted);
        const status = yield* Effect.forkChild(
          Effect.gen(function* () {
            yield* Deferred.succeed(statusStarted, undefined);
            return yield* withArchiveActionWorkflowEffect(
              Effect.gen(function* () {
                const action = (yield* getArchiveActionEffect(queued.actionId))!;
                return action.status === "copy_claimed"
                  ? yield* advanceCopyActionEffect(
                      action,
                      methods,
                      false,
                      undefined,
                      false,
                    )
                  : action;
              }),
            );
          }),
        );
        yield* Deferred.await(statusStarted);
        expect((yield* getArchiveActionEffect(queued.actionId))?.status).toBe(
          "copy_claimed",
        );
        expect(methods.reconcileExactCopyEffect).not.toHaveBeenCalled();
        yield* Deferred.succeed(releaseCopy, undefined);
        yield* Fiber.join(worker);
        expect((yield* Fiber.join(status)).status).toBe("archived");
        expect((yield* getArchiveActionEffect(queued.actionId))?.destination).toEqual(
          destination,
        );
      }),
    );
  });

  it("restores only the recorded Archive UID through the same scoped stages", async () => {
    const queued = await run(
      queueArchiveActionEffect("copy-restore", identity("copy-restore")),
    );
    const destination = { folder: "Archive", uidValidity: "20", uid: 12 };
    const archived = await run(
      updateArchiveActionEffect(queued.actionId, "queued", "archived", {
        snapshot: {
          sourceHash: "abc",
          flags: ["\\Flagged"],
          strategy: "uidplus_copy",
          targetFolder: "Archive",
        },
        destination,
      }),
    );
    expect(archived.status).toBe("archived");
    let sourceState: "marked" | "absent" = "marked";
    const methods = transport({
      copyExactArchiveMessageEffect: vi.fn(() =>
        Effect.succeed({
          folder: "INBOX",
          uidValidity: "10",
          uid: 8,
        }),
      ),
      reconcileExactCopyEffect: vi.fn(() =>
        Effect.succeed({
          state: "moved",
          destination: { folder: "INBOX", uidValidity: "10", uid: 8 },
          snapshot: { sourceHash: "abc", flags: ["\\Seen"] },
        }),
      ),
      markExactArchiveSourceDeletedEffect: vi.fn(() => Effect.succeed(true)),
      inspectExactDeletedSourceEffect: vi.fn(() => Effect.sync(() => sourceState)),
      expungeExactArchiveSourceEffect: vi.fn(() =>
        Effect.sync(() => {
          sourceState = "absent";
          return true;
        }),
      ),
    });
    const restored = await run(restoreArchivedActionEffect(queued.actionId, methods));
    expect(restored.status).toBe("restored");
    expect(restored.restoredLocation).toEqual({
      folder: "INBOX",
      uidValidity: "10",
      uid: 8,
    });
    expect(methods.copyExactArchiveMessageEffect).toHaveBeenCalledWith(
      { ...destination, messageId: queued.identity.messageId },
      "INBOX",
      expect.objectContaining({ flags: ["\\Seen"] }),
    );
    expect(methods.expungeExactArchiveSourceEffect).toHaveBeenCalledWith(
      { ...destination, messageId: queued.identity.messageId },
      restored.restoredLocation,
      expect.objectContaining({ flags: ["\\Seen"] }),
    );
  });

  it("reconciles claimed STORE and EXPUNGE on status without mutating", async () => {
    const queued = await run(
      queueArchiveActionEffect("copy-status", identity("copy-status")),
    );
    const destination = { folder: "Archive", uidValidity: "20", uid: 12 };
    const snapshot = {
      sourceHash: "abc",
      flags: ["\\Flagged"],
      strategy: "uidplus_copy" as const,
      targetFolder: "Archive",
    };
    const claimed = await run(
      updateArchiveActionEffect(queued.actionId, "queued", "delete_claimed", {
        snapshot,
        destination,
      }),
    );
    let state: "marked" | "absent" = "marked";
    const methods = transport({
      copyExactArchiveMessageEffect: vi.fn(() => Effect.succeed(destination)),
      reconcileExactCopyEffect: vi.fn(() =>
        Effect.succeed({ state: "uncertain" as const }),
      ),
      markExactArchiveSourceDeletedEffect: vi.fn(() => Effect.succeed(true)),
      inspectExactDeletedSourceEffect: vi.fn(() => Effect.sync(() => state)),
      expungeExactArchiveSourceEffect: vi.fn(() => Effect.succeed(true)),
    });
    expect(
      (await run(advanceCopyActionEffect(claimed, methods, false, undefined, false)))
        .status,
    ).toBe("delete_claimed");
    expect(methods.expungeExactArchiveSourceEffect).not.toHaveBeenCalled();
    const expungeClaimed = await run(
      updateArchiveActionEffect(queued.actionId, "delete_claimed", "expunge_claimed"),
    );
    state = "absent";
    expect(
      (
        await run(
          advanceCopyActionEffect(expungeClaimed, methods, false, undefined, false),
        )
      ).status,
    ).toBe("archived");
    expect(methods.markExactArchiveSourceDeletedEffect).not.toHaveBeenCalled();
    expect(methods.expungeExactArchiveSourceEffect).not.toHaveBeenCalled();
  });
  it("reserves exact Message-ID, rejects a conflicting key, and cancels before MOVE", async () => {
    const original = identity("queue");
    const first = await run(queueArchiveActionEffect("queue-key", original));
    expect(first.status).toBe("queued");
    expect(await run(isArchiveActionMessageEffect(original.messageId, original))).toBe(
      false,
    );
    expect(await run(queueArchiveActionEffect("queue-key", original))).toEqual(first);
    await expect(
      run(queueArchiveActionEffect("queue-key", identity("other"))),
    ).rejects.toThrow(/different message/);
    await expect(
      run(queueArchiveActionEffect("another-key", original)),
    ).rejects.toThrow(/already has an archive action/);
    await run(updateArchiveActionEffect(first.actionId, "queued", "cancelled"));
    expect((await run(getArchiveActionEffect(first.actionId)))?.status).toBe(
      "cancelled",
    );
    const replacement = await run(
      queueArchiveActionEffect("replacement-key", original),
    );
    expect(replacement.status).toBe("queued");
    await run(updateArchiveActionEffect(replacement.actionId, "queued", "cancelled"));
  });

  it("records the claim before MOVE and never repeats an uncertain MOVE", async () => {
    const queued = await run(
      queueArchiveActionEffect("uncertain-key", identity("uncertain")),
    );
    const methods = transport({
      moveArchiveMessageEffect: vi.fn(() => Effect.fail(new Error("connection lost"))),
      reconcileArchiveMessageEffect: vi.fn(() =>
        Effect.succeed({ state: "uncertain" }),
      ),
    });
    const first = await run(processArchiveActionEffect(queued, methods));
    expect(first.status).toBe("uncertain");
    expect(first.snapshot?.sourceHash).toBe("abc");
    expect(methods.moveArchiveMessageEffect).toHaveBeenCalledOnce();
    await run(processQueuedArchiveActionsEffect(methods));
    const moveCalls = vi.mocked(methods.moveArchiveMessageEffect!).mock
      .calls as unknown as Array<[typeof queued.identity]>;
    expect(
      moveCalls.filter(([source]) => source.messageId === queued.identity.messageId),
    ).toHaveLength(1);
    await run(updateArchiveActionEffect(queued.actionId, "uncertain", "failed"));
  });

  it("keeps a cancelled action out of the worker sweep", async () => {
    const queued = await run(
      queueArchiveActionEffect("cancel-sweep", identity("cancel-sweep")),
    );
    await run(updateArchiveActionEffect(queued.actionId, "queued", "cancelled"));
    const methods = transport();
    await run(processQueuedArchiveActionsEffect(methods));
    expect(methods.moveArchiveMessageEffect).not.toHaveBeenCalled();
  });

  it("suppresses only action-created Archive and restored Inbox origins", async () => {
    const source = identity("suppression");
    const queued = await run(queueArchiveActionEffect("suppression-key", source));
    const archive = { folder: "Archive", uidValidity: "20", uid: 12 };
    const restored = { folder: "INBOX", uidValidity: "10", uid: 8 };
    expect(await run(isArchiveActionMessageEffect(source.messageId, archive))).toBe(
      false,
    );
    await run(
      updateArchiveActionEffect(queued.actionId, "queued", "claimed", {
        snapshot: { sourceHash: "abc", flags: [] },
      }),
    );
    expect(await run(isArchiveActionMessageEffect(source.messageId, source))).toBe(
      false,
    );
    expect(await run(isArchiveActionMessageEffect(source.messageId, archive))).toBe(
      true,
    );
    await run(
      updateArchiveActionEffect(queued.actionId, "claimed", "archived", {
        destination: archive,
      }),
    );
    expect(await run(isArchiveActionMessageEffect(source.messageId, archive))).toBe(
      true,
    );
    expect(
      await run(
        isArchiveActionMessageEffect(source.messageId, { ...archive, uid: 13 }),
      ),
    ).toBe(false);
    expect(await run(isArchiveActionMessageEffect(source.messageId, source))).toBe(
      false,
    );
    await run(
      updateArchiveActionEffect(queued.actionId, "archived", "restore_claimed"),
    );
    expect(await run(isArchiveActionMessageEffect(source.messageId, restored))).toBe(
      true,
    );
    await run(
      updateArchiveActionEffect(queued.actionId, "restore_claimed", "restored", {
        restoredLocation: restored,
      }),
    );
    expect(await run(isArchiveActionMessageEffect(source.messageId, restored))).toBe(
      true,
    );
    expect(await run(isArchiveActionMessageEffect(source.messageId, source))).toBe(
      false,
    );
  });

  it("keeps a delayed restore event suppressed after immediate requeue", async () => {
    const initial = identity("requeue-history");
    const first = await run(queueArchiveActionEffect("requeue-history-first", initial));
    const archived = { folder: "Archive", uidValidity: "20", uid: 42 };
    const restored = { folder: "INBOX", uidValidity: "10", uid: 43 };
    await run(
      updateArchiveActionEffect(first.actionId, "queued", "claimed", {
        snapshot: { sourceHash: "abc", flags: [] },
      }),
    );
    await run(
      updateArchiveActionEffect(first.actionId, "claimed", "archived", {
        destination: archived,
      }),
    );
    await run(updateArchiveActionEffect(first.actionId, "archived", "restore_claimed"));
    await run(
      updateArchiveActionEffect(first.actionId, "restore_claimed", "restored", {
        restoredLocation: restored,
      }),
    );
    const second = await run(
      queueArchiveActionEffect("requeue-history-second", {
        ...restored,
        messageId: initial.messageId,
      }),
    );
    expect(second.status).toBe("queued");
    expect(await run(isArchiveActionMessageEffect(initial.messageId, initial))).toBe(
      false,
    );
    expect(await run(isArchiveActionMessageEffect(initial.messageId, restored))).toBe(
      true,
    );
    expect(await run(isArchiveActionMessageEffect(initial.messageId, archived))).toBe(
      true,
    );
    await run(updateArchiveActionEffect(second.actionId, "queued", "cancelled"));
  });

  it("uses the latest reservation marker when an older row has no history key", async () => {
    const source = identity("legacy-marker");
    const action = await run(queueArchiveActionEffect("legacy-marker-key", source));
    const archived = { folder: "Archive", uidValidity: "20", uid: 44 };
    await run(
      updateArchiveActionEffect(action.actionId, "queued", "claimed", {
        snapshot: { sourceHash: "abc", flags: [] },
      }),
    );
    await run(
      updateArchiveActionEffect(action.actionId, "claimed", "archived", {
        destination: archived,
      }),
    );
    const digest = createHash("sha256").update(source.messageId).digest("hex");
    await run(
      Docstore.use((service) => service.deleteDoc(`email-archive:history:${digest}`)),
    );
    expect(await run(isArchiveActionMessageEffect(source.messageId, archived))).toBe(
      true,
    );
  });

  it("protects exact archived UID and skips ambiguous Archive auto-read", async () => {
    const source = identity("auto-read-protection");
    const queued = await run(queueArchiveActionEffect("auto-read-protection", source));
    expect(await run(archiveAutoReadProtectionEffect("Archive", "20"))).toMatchObject({
      skip: false,
    });
    await run(
      updateArchiveActionEffect(queued.actionId, "queued", "claimed", {
        snapshot: { sourceHash: "abc", flags: [] },
      }),
    );
    const uncertainProtection = await run(
      archiveAutoReadProtectionEffect("Archive", "20"),
    );
    expect(uncertainProtection.skip).toBe(false);
    expect(uncertainProtection.fallbackMessageIds).toContain(source.messageId);
    await run(
      updateArchiveActionEffect(queued.actionId, "claimed", "archived", {
        destination: { folder: "Archive", uidValidity: "20", uid: 12 },
      }),
    );
    const protectedUids = await run(archiveAutoReadProtectionEffect("Archive", "20"));
    expect(protectedUids.skip).toBe(false);
    expect(protectedUids.excludedUids.has(12)).toBe(true);
    const staleProtection = await run(archiveAutoReadProtectionEffect("Archive", "21"));
    expect(staleProtection.skip).toBe(false);
    expect(staleProtection.fallbackMessageIds).toContain(source.messageId);
  });

  it("fails a missing native MOVE capability without trying a mutation", async () => {
    const queued = await run(queueArchiveActionEffect("no-move", identity("no-move")));
    const methods = transport({
      inspectArchiveMessageEffect: vi.fn(() =>
        Effect.fail(new Error("Server does not advertise MOVE")),
      ),
    });
    const result = await run(processArchiveActionEffect(queued, methods));
    expect(result).toMatchObject({
      status: "failed",
      reason: "native_move_unavailable",
      attempts: 1,
    });
    expect(methods.moveArchiveMessageEffect).not.toHaveBeenCalled();
  });

  it("reconciles a claimed action after restart without repeating MOVE", async () => {
    const queued = await run(
      queueArchiveActionEffect("claimed-restart", identity("claimed-restart")),
    );
    await run(
      updateArchiveActionEffect(queued.actionId, "queued", "claimed", {
        snapshot: { sourceHash: "abc", flags: ["\\Flagged"] },
      }),
    );
    const methods = transport();
    await run(processQueuedArchiveActionsEffect(methods));
    expect(methods.moveArchiveMessageEffect).not.toHaveBeenCalled();
    expect((await run(getArchiveActionEffect(queued.actionId)))?.status).toBe(
      "archived",
    );
  });

  it("archives with verified receipt and restores with current Archive flags", async () => {
    const queued = await run(
      queueArchiveActionEffect("restore-key", identity("restore")),
    );
    const methods = transport();
    const archived = await run(processArchiveActionEffect(queued, methods));
    expect(archived.status).toBe("archived");
    const restored = await run(restoreArchivedActionEffect(queued.actionId, methods));
    expect(restored.status).toBe("restored");
    expect(methods.verifyArchiveLocationEffect).toHaveBeenLastCalledWith(
      { folder: "INBOX", uidValidity: "10", uid: 8 },
      identity("restore").messageId,
      "abc",
      ["\\Seen"],
    );
  });

  it("leaves restore uncertain when the Archive source is not confirmed gone", async () => {
    const queued = await run(
      queueArchiveActionEffect("restore-uncertain", identity("restore-uncertain")),
    );
    const methods = transport({
      reconcileRestoreMessageEffect: vi.fn(() =>
        Effect.succeed({ state: "uncertain" }),
      ),
    });
    await run(processArchiveActionEffect(queued, methods));
    const restored = await run(restoreArchivedActionEffect(queued.actionId, methods));
    expect(restored.status).toBe("restore_uncertain");
    expect(methods.restoreArchiveMessageEffect).toHaveBeenCalledOnce();
  });
});
