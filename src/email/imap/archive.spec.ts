import { Effect } from "effect";
import { describe, expect, it, vi } from "vitest";
import {
  inspectArchiveSourceEffect,
  moveArchiveMessageEffect,
  reconcileArchiveMessageEffect,
  restoreArchiveMessageEffect,
  verifyArchiveLocationEffect,
  type ArchiveClient,
} from "./archive.js";

const identity = {
  folder: "INBOX",
  uidValidity: "10",
  uid: 7,
  messageId: "<one@example.test>",
};

function fixture(moveSupported = true) {
  const source = Buffer.from("Message-ID: <one@example.test>\r\n\r\nprivate body");
  const message = {
    source,
    flags: new Set(["\\Seen", "\\Flagged"]),
    envelope: { messageId: identity.messageId },
  };
  const folders = {
    INBOX: new Map<number, typeof message>([[7, message]]),
    "iCloud Archive": new Map<number, typeof message>(),
  };
  let selected: keyof typeof folders = "INBOX";
  const client = {
    capabilities: new Map(moveSupported ? [["MOVE", true]] : []),
    mailbox: { path: selected as string, uidValidity: 10n },
    list: vi.fn(async () => [
      { path: "INBOX" },
      { path: "iCloud Archive", specialUse: "\\Archive" },
    ]),
    getMailboxLock: vi.fn(async (path: keyof typeof folders) => {
      selected = path;
      client.mailbox = {
        path,
        uidValidity: path === "INBOX" ? 10n : 20n,
      };
      return { release: vi.fn() };
    }),
    fetchOne: vi.fn(async (uid: string) => folders[selected].get(Number(uid)) || false),
    search: vi.fn(async () => [...folders[selected].keys()]),
    messageMove: vi.fn(async (uids: number[], destination: keyof typeof folders) => {
      const item = folders[selected].get(uids[0]);
      if (!item) return false;
      folders[selected].delete(uids[0]);
      const newUid = destination === "INBOX" ? 8 : 12;
      folders[destination].set(newUid, item);
      return {
        uidValidity: destination === "INBOX" ? 10n : 20n,
        uidMap: new Map([[uids[0], newUid]]),
      };
    }),
  };
  return { client: client as ArchiveClient, spies: client, folders, message };
}

describe("exact native IMAP archive", () => {
  it("refuses a server without MOVE before any mailbox mutation", async () => {
    const { client, spies } = fixture(false);
    await expect(
      Effect.runPromise(inspectArchiveSourceEffect(client, identity)),
    ).rejects.toThrow(/does not advertise MOVE/);
    await expect(
      Effect.runPromise(moveArchiveMessageEffect(client, identity, "unknown")),
    ).rejects.toThrow(/does not advertise MOVE/);
    expect(spies.messageMove).not.toHaveBeenCalled();
  });

  it("requires the exact Inbox UIDVALIDITY and Message-ID", async () => {
    const { client, spies } = fixture();
    await expect(
      Effect.runPromise(
        inspectArchiveSourceEffect(client, { ...identity, uidValidity: "11" }),
      ),
    ).rejects.toThrow(/UIDVALIDITY changed/);
    await expect(
      Effect.runPromise(
        inspectArchiveSourceEffect(client, {
          ...identity,
          messageId: "<other@example.test>",
        }),
      ),
    ).rejects.toThrow(/different Message-ID/);
    expect(spies.messageMove).not.toHaveBeenCalled();
  });

  it("rejects a changed MIME source and an existing identical Archive copy", async () => {
    const { client, spies, folders, message } = fixture();
    await expect(
      Effect.runPromise(moveArchiveMessageEffect(client, identity, "wrong-hash")),
    ).rejects.toThrow(/Source changed since reservation/);
    folders["iCloud Archive"].set(11, message);
    await expect(
      Effect.runPromise(inspectArchiveSourceEffect(client, identity)),
    ).rejects.toThrow(/Matching Archive copy already exists/);
    await expect(
      Effect.runPromise(moveArchiveMessageEffect(client, identity, "wrong-hash")),
    ).rejects.toThrow(/Matching Archive copy already exists/);
    expect(spies.messageMove).not.toHaveBeenCalled();
  });

  it("refuses an unconfirmed Archive duplicate search", async () => {
    const { client, spies } = fixture();
    spies.search.mockResolvedValue(false as never);
    await expect(
      Effect.runPromise(inspectArchiveSourceEffect(client, identity)),
    ).rejects.toThrow(/search was not confirmed/);
    expect(spies.messageMove).not.toHaveBeenCalled();
  });

  it("treats failed source or candidate reads as uncertain", async () => {
    const { client, spies, folders, message } = fixture();
    const snapshot = await Effect.runPromise(
      inspectArchiveSourceEffect(client, identity),
    );
    folders["iCloud Archive"].set(12, message);
    const originalFetch = spies.fetchOne.getMockImplementation()!;
    spies.fetchOne.mockImplementation(async (uid: string) => {
      if (spies.mailbox.path === "INBOX") throw new Error("source read failed");
      return originalFetch(uid);
    });
    expect(
      await Effect.runPromise(
        reconcileArchiveMessageEffect(client, identity, snapshot.sourceHash),
      ),
    ).toEqual({ state: "uncertain" });
    spies.fetchOne.mockImplementation(async (uid: string) => {
      if (spies.mailbox.path === "iCloud Archive")
        throw new Error("candidate read failed");
      return originalFetch(uid);
    });
    await expect(
      Effect.runPromise(
        reconcileArchiveMessageEffect(client, identity, snapshot.sourceHash),
      ),
    ).rejects.toThrow(/candidate read failed/);
  });

  it("never treats a changed source UIDVALIDITY as proof of MOVE", async () => {
    const { client, spies, folders, message } = fixture();
    const snapshot = await Effect.runPromise(
      inspectArchiveSourceEffect(client, identity),
    );
    folders["iCloud Archive"].set(12, message);
    const originalLock = spies.getMailboxLock.getMockImplementation()!;
    spies.getMailboxLock.mockImplementation(
      async (path: "INBOX" | "iCloud Archive") => {
        const lock = await originalLock(path);
        if (path === "INBOX") spies.mailbox.uidValidity = 99n;
        return lock;
      },
    );
    expect(
      await Effect.runPromise(
        reconcileArchiveMessageEffect(client, identity, snapshot.sourceHash),
      ),
    ).toEqual({ state: "uncertain" });
  });

  it("moves only the exact UID and verifies content and flags in Archive", async () => {
    const { client, spies, folders } = fixture();
    const snapshot = await Effect.runPromise(
      inspectArchiveSourceEffect(client, identity),
    );
    const moved = await Effect.runPromise(
      moveArchiveMessageEffect(client, identity, snapshot.sourceHash),
    );
    expect(spies.messageMove).toHaveBeenCalledWith([7], "iCloud Archive", {
      uid: true,
    });
    expect(moved.destination).toEqual({
      folder: "iCloud Archive",
      uidValidity: "20",
      uid: 12,
    });
    expect(folders.INBOX.has(7)).toBe(false);
    expect(
      await Effect.runPromise(
        verifyArchiveLocationEffect(
          client,
          moved.destination,
          identity.messageId,
          snapshot.sourceHash,
          snapshot.flags,
        ),
      ),
    ).toBe(true);
    expect(
      await Effect.runPromise(
        reconcileArchiveMessageEffect(client, identity, snapshot.sourceHash),
      ),
    ).toMatchObject({ state: "moved", destination: moved.destination });
    const restored = await Effect.runPromise(
      restoreArchiveMessageEffect(
        client,
        identity,
        moved.destination,
        snapshot.sourceHash,
      ),
    );
    expect(restored.destination.folder).toBe("INBOX");
    expect(folders.INBOX.has(8)).toBe(true);
  });
});
