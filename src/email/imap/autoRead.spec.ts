import { describe, expect, it, vi } from "vitest";
import { Effect } from "effect";
import {
  type AutoReadClient,
  type AutoReadMailbox,
  markRecentUnreadReadEffect,
  selectAutoReadFolders,
} from "./autoRead.js";

const mailbox = (
  path: string,
  specialUse?: string,
  flags = new Set<string>(),
): AutoReadMailbox => ({ path, specialUse, flags });

function makeClient(overrides: Partial<AutoReadClient> = {}): AutoReadClient {
  return {
    list: vi.fn(async () => []),
    getMailboxLock: vi.fn(async () => ({ release: vi.fn() })),
    search: vi.fn(async () => []),
    messageFlagsAdd: vi.fn(async () => true),
    ...overrides,
  };
}

describe("selectAutoReadFolders", () => {
  it("selects localized paths by exact special-use role", () => {
    expect(
      selectAutoReadFolders([
        mailbox("Archiv", "\\Archive"),
        mailbox("Spam", "\\Junk"),
        mailbox("Papierkorb", "\\Trash"),
      ]),
    ).toEqual(["Archiv", "Spam", "Papierkorb"]);
  });

  it("ignores other roles, noselection mailboxes, and duplicate paths", () => {
    expect(
      selectAutoReadFolders([
        mailbox("All Mail", "\\All"),
        mailbox("Archive", "\\Archive", new Set(["\\Noselect"])),
        mailbox("Archive", "\\Archive"),
        mailbox("Archive", "\\Archive"),
        mailbox("Inbox"),
      ]),
    ).toEqual(["Archive"]);
  });
});

describe("markRecentUnreadRead", () => {
  it("does nothing for an empty unread search", async () => {
    const client = makeClient();
    const logger = { warn: vi.fn(() => Effect.void) };

    await Effect.runPromise(markRecentUnreadReadEffect(client, ["Archive"], logger));

    expect(client.messageFlagsAdd).not.toHaveBeenCalled();
    expect(client.getMailboxLock).toHaveBeenCalledWith("Archive", { readOnly: false });
  });

  it("searches recent unread mail and adds Seen by UID silently", async () => {
    const now = new Date("2026-08-21T18:30:00.000Z");
    vi.useFakeTimers();
    vi.setSystemTime(now);
    const client = makeClient({
      search: vi.fn(async () => [4, 9]),
    });
    const logger = { warn: vi.fn(() => Effect.void) };

    try {
      await Effect.runPromise(markRecentUnreadReadEffect(client, ["Archive"], logger));
    } finally {
      vi.useRealTimers();
    }

    expect(client.search).toHaveBeenCalledWith(
      { seen: false, since: new Date("2026-08-20T18:30:00.000Z") },
      { uid: true },
    );
    expect(client.messageFlagsAdd).toHaveBeenCalledWith([4, 9], ["\\Seen"], {
      uid: true,
      silent: true,
    });
  });

  it("releases the writable lock when marking succeeds or fails", async () => {
    const successRelease = vi.fn();
    const successClient = makeClient({
      getMailboxLock: vi.fn(async () => ({ release: successRelease })),
      search: vi.fn(async () => [3]),
    });
    const logger = { warn: vi.fn(() => Effect.void) };

    await Effect.runPromise(
      markRecentUnreadReadEffect(successClient, ["Archive"], logger),
    );

    expect(successRelease).toHaveBeenCalledOnce();

    const errorRelease = vi.fn();
    const errorClient = makeClient({
      getMailboxLock: vi.fn(async () => ({ release: errorRelease })),
      search: vi.fn(async () => {
        throw new Error("search failed");
      }),
    });

    await Effect.runPromise(
      markRecentUnreadReadEffect(errorClient, ["Archive"], logger),
    );

    expect(errorRelease).toHaveBeenCalledOnce();
    expect(logger.warn).toHaveBeenCalledWith(
      expect.stringContaining('folder "Archive"'),
    );
  });

  it("continues after one folder fails", async () => {
    const client = makeClient({
      getMailboxLock: vi.fn(async (folder: string) => {
        if (folder === "Junk") throw new Error("unavailable");
        return { release: vi.fn() };
      }),
      search: vi.fn(async () => [8]),
    });
    const logger = { warn: vi.fn(() => Effect.void) };

    await Effect.runPromise(
      markRecentUnreadReadEffect(client, ["Junk", "Trash"], logger),
    );

    expect(client.search).toHaveBeenCalledTimes(1);
    expect(client.messageFlagsAdd).toHaveBeenCalledWith([8], ["\\Seen"], {
      uid: true,
      silent: true,
    });
    expect(logger.warn).toHaveBeenCalledWith(expect.stringContaining('folder "Junk"'));
  });

  it("continues after a selected folder search fails", async () => {
    let selectedFolder = "";
    const client = makeClient({
      getMailboxLock: vi.fn(async (folder: string) => {
        selectedFolder = folder;
        return { release: vi.fn() };
      }),
      search: vi.fn(async () => {
        if (selectedFolder === "Junk") throw new Error("search failed");
        return [12];
      }),
    });
    const logger = { warn: vi.fn(() => Effect.void) };

    await Effect.runPromise(
      markRecentUnreadReadEffect(client, ["Junk", "Trash"], logger),
    );

    expect(client.search).toHaveBeenCalledTimes(2);
    expect(client.messageFlagsAdd).toHaveBeenCalledOnce();
    expect(client.messageFlagsAdd).toHaveBeenCalledWith([12], ["\\Seen"], {
      uid: true,
      silent: true,
    });
    expect(logger.warn).toHaveBeenCalledWith(expect.stringContaining('folder "Junk"'));
  });
});
