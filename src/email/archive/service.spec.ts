import { createHash } from "node:crypto";
import { Docstore } from "@micthiesen/mitools/docstore";
import { Effect } from "effect";
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
  processArchiveActionEffect,
  processQueuedArchiveActionsEffect,
  restoreArchivedActionEffect,
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
    expect(methods.moveArchiveMessageEffect).toHaveBeenCalledOnce();
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
