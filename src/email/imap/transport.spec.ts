import { Logger, type NamedLogger } from "@micthiesen/mitools/logging";
import { Deferred, Effect, Fiber, ManagedRuntime } from "effect";
import { describe, expect, it, vi } from "vitest";

const imapFlowMock = vi.hoisted(() => vi.fn());
vi.mock("imapflow", () => ({ ImapFlow: imapFlowMock }));

import { ImapTransport } from "./transport.js";
import { BoundedReadCache } from "./readCache.js";

const runtime = ManagedRuntime.make(Logger.layer());
const runPromise = runtime.runPromise.bind(runtime);
const runFork = runtime.runFork.bind(runtime);

const logger = {
  debug: vi.fn(() => Effect.void),
  info: vi.fn(() => Effect.void),
  warn: vi.fn(() => Effect.void),
  error: vi.fn(() => Effect.void),
  extend: vi.fn(),
} as unknown as NamedLogger;

describe("ImapTransport mailbox serialization", () => {
  it("batches UID downloads and reuses bounded parsed/search results", async () => {
    const mailbox = { path: "INBOX", uidValidity: 12 };
    const source = (subject: string) =>
      Buffer.from(
        `Message-ID: <${subject}@example.test>\r\nSubject: ${subject}\r\nFrom: sender@example.test\r\nDate: Tue, 01 Sep 2026 10:00:00 +0000\r\n\r\nbody`,
      );
    const client = {
      usable: true,
      mailbox,
      getMailboxLock: vi.fn(async (folder: string) => {
        mailbox.path = folder;
        return { release: vi.fn() };
      }),
      search: vi.fn(async () => [1, 2]),
      fetch: vi.fn(async function* (uids: number[]) {
        for (const uid of uids)
          yield {
            uid,
            source: source(`message-${uid}`),
            internalDate: new Date(`2026-09-01T10:00:0${uid}Z`),
          };
      }),
      mailboxOpen: vi.fn(async (folder: string) => {
        mailbox.path = folder;
      }),
    };
    const transport = new ImapTransport({ user: "u", pass: "p" }, logger);
    Object.assign(transport as object, {
      client,
      parsedMessageCache: new BoundedReadCache(1, 1_000_000, 300_000),
    });

    const options = { folder: "archive" as const, limit: 2 };
    const first = await runPromise(transport.searchEmailsEffect(options));
    const second = await runPromise(transport.searchEmailsEffect(options));
    const fresh = await runPromise(
      transport.searchEmailsEffect({ ...options, fresh: true }),
    );

    expect(first.map((email) => email.subject)).toEqual(["message-2", "message-1"]);
    expect(second).toEqual(first);
    expect(fresh).toEqual(first);
    expect(client.fetch).toHaveBeenCalledTimes(2);
    expect(client.fetch).toHaveBeenCalledWith(
      [2, 1],
      { source: true, internalDate: true },
      { uid: true },
    );
    expect(client.search).toHaveBeenCalledTimes(2);

    await runPromise(transport.searchEmailsEffect({ ...options, limit: 1 }));
    const mixed = await runPromise(
      transport.searchEmailsEffect({ ...options, query: "body" }),
    );
    expect(mixed.map((email) => email.subject)).toEqual(["message-2", "message-1"]);
    expect(client.fetch).toHaveBeenLastCalledWith(
      [1],
      { source: true, internalDate: true },
      { uid: true },
    );
  });

  it("serves repeated direct reads from a short cache and honors fresh", async () => {
    const mailbox = { path: "INBOX", uidValidity: 12 };
    const client = {
      usable: true,
      mailbox,
      getMailboxLock: vi.fn(async (folder: string) => {
        mailbox.path = folder;
        return { release: vi.fn() };
      }),
      search: vi.fn(async () => [7]),
      fetchOne: vi.fn(
        async (_range: string, query: { uid?: boolean; source?: boolean }) =>
          query.source
            ? {
                uid: 7,
                source: Buffer.from(
                  "Message-ID: <direct@example.test>\r\nSubject: direct\r\nFrom: sender@example.test\r\n\r\nbody",
                ),
                internalDate: new Date("2026-09-01T10:00:00Z"),
              }
            : { uid: 7 },
      ),
      mailboxOpen: vi.fn(async (folder: string) => {
        mailbox.path = folder;
      }),
    };
    const transport = new ImapTransport({ user: "u", pass: "p" }, logger);
    Object.assign(transport as object, { client });

    const first = await runPromise(
      transport.fetchEmailByIdEffect("<direct@example.test>"),
    );
    const second = await runPromise(
      transport.fetchEmailByIdEffect("<direct@example.test>"),
    );
    const fresh = await runPromise(
      transport.fetchEmailByIdEffect("<direct@example.test>", { fresh: true }),
    );

    expect(first?.subject).toBe("direct");
    expect(second).toEqual(first);
    expect(fresh).toEqual(first);
    expect(client.search).toHaveBeenCalledTimes(2);
    expect(client.fetchOne).toHaveBeenCalledTimes(2);
  });

  it("serializes complete select/use/restore operations", async () => {
    const firstLockRequested = await Effect.runPromise(Deferred.make<void>());
    const releaseFirstLock = await Effect.runPromise(Deferred.make<void>());
    let active = 0;
    let maxActive = 0;
    let lockRequests = 0;
    const mailbox = { path: "INBOX", uidValidity: 1 };
    const client = {
      usable: true,
      mailbox,
      getMailboxLock: vi.fn(async (folder: string) => {
        lockRequests++;
        active++;
        maxActive = Math.max(maxActive, active);
        mailbox.path = folder;
        if (lockRequests === 1) {
          await Effect.runPromise(Deferred.succeed(firstLockRequested, undefined));
          await Effect.runPromise(Deferred.await(releaseFirstLock));
        }
        return { release: () => active-- };
      }),
      search: vi.fn(async () => []),
      mailboxOpen: vi.fn(async (folder: string) => {
        mailbox.path = folder;
      }),
    };
    const transport = new ImapTransport({ user: "u", pass: "p" }, logger);
    Object.assign(transport as object, { client });

    const searches = Effect.all(
      [
        transport.searchEmailsEffect({ folder: "archive", limit: 1 }),
        transport.searchEmailsEffect({ folder: "archive", limit: 1 }),
      ],
      { concurrency: "unbounded" },
    );
    const fiber = runFork(searches);
    await Effect.runPromise(Deferred.await(firstLockRequested));
    await Effect.runPromise(Deferred.succeed(releaseFirstLock, undefined));
    await Effect.runPromise(Fiber.join(fiber));

    expect(maxActive).toBe(1);
    expect(mailbox.path).toBe("INBOX");
    expect(client.mailboxOpen).toHaveBeenCalledOnce();
  });

  it("shares concurrent connect attempts", async () => {
    let finish: (() => void) | undefined;
    const transport = new ImapTransport({ user: "u", pass: "p" }, logger);
    const connect = vi.fn(() =>
      Effect.callback<void>((resume) => {
        finish = () => resume(Effect.void);
      }),
    );
    const internal = transport as unknown as {
      connectEffect: Effect.Effect<void>;
      connectSingleFlightEffect: Effect.Effect<void>;
    };
    internal.connectEffect = Effect.suspend(connect);

    const first = Effect.runPromise(internal.connectSingleFlightEffect);
    const second = Effect.runPromise(internal.connectSingleFlightEffect);
    await vi.waitFor(() => expect(connect).toHaveBeenCalledOnce());

    finish?.();
    await Promise.all([first, second]);
  });

  it("queues shutdown behind an active mailbox operation", async () => {
    let finish: (() => void) | undefined;
    const active = new Promise<void>((resolve) => {
      finish = resolve;
    });
    const logout = vi.fn(async () => {});
    const transport = new ImapTransport({ user: "u", pass: "p" }, logger);
    const internal = transport as unknown as {
      client: { logout: typeof logout; close: () => void };
      runSerializedEffect: <A>(
        name: string,
        effect: Effect.Effect<A>,
      ) => Effect.Effect<A>;
    };
    internal.client = { logout, close: vi.fn() };

    const operationFiber = Effect.runFork(
      internal.runSerializedEffect(
        "test operation",
        Effect.promise(() => active),
      ),
    );
    const stop = runPromise(transport.stopEffect);
    await Effect.runPromise(Effect.yieldNow);
    expect(logout).not.toHaveBeenCalled();

    finish?.();
    await Effect.runPromise(Fiber.join(operationFiber));
    await stop;
    expect(logout).toHaveBeenCalledOnce();
  });

  it("closes the local client when selecting INBOX fails", async () => {
    const client = makeConnectClient({
      mailboxOpen: vi.fn().mockRejectedValue(new Error("select failed")),
    });
    imapFlowMock.mockImplementationOnce(function ImapFlow() {
      return client;
    });
    const transport = new ImapTransport({ user: "u", pass: "p" }, logger);
    const connectEffect = (
      transport as unknown as { connectEffect: Effect.Effect<void, Error> }
    ).connectEffect;

    await expect(runPromise(connectEffect)).rejects.toThrow("select failed");
    expect(client.close).toHaveBeenCalledOnce();
    expect((transport as unknown as { client: unknown }).client).toBeNull();
  });

  it("closes the local client when connect fails", async () => {
    const client = makeConnectClient({
      connect: vi.fn().mockRejectedValue(new Error("connect failed")),
    });
    imapFlowMock.mockImplementationOnce(function ImapFlow() {
      return client;
    });
    const transport = new ImapTransport({ user: "u", pass: "p" }, logger);
    const connectEffect = (
      transport as unknown as { connectEffect: Effect.Effect<void, Error> }
    ).connectEffect;

    await expect(runPromise(connectEffect)).rejects.toThrow("connect failed");
    expect(client.close).toHaveBeenCalledOnce();
    expect(client.mailboxOpen).not.toHaveBeenCalled();
    expect((transport as unknown as { client: unknown }).client).toBeNull();
  });

  it("closes a non-signal-aware local client when connect is interrupted", async () => {
    let finishConnect: (() => void) | undefined;
    const client = makeConnectClient({
      connect: vi.fn(
        () =>
          new Promise<void>((resolve) => {
            finishConnect = resolve;
          }),
      ),
    });
    imapFlowMock.mockImplementationOnce(function ImapFlow() {
      return client;
    });
    const transport = new ImapTransport({ user: "u", pass: "p" }, logger);
    const connectEffect = (
      transport as unknown as { connectEffect: Effect.Effect<void, Error> }
    ).connectEffect;

    const fiber = runFork(connectEffect);
    await vi.waitFor(() => expect(client.connect).toHaveBeenCalledOnce());
    await Effect.runPromise(Fiber.interrupt(fiber));
    expect(client.close).toHaveBeenCalledOnce();

    // Resolving the underlying Promise later cannot transfer the already
    // released client into the transport.
    finishConnect?.();
    await Effect.runPromise(Effect.yieldNow);
    expect(client.mailboxOpen).not.toHaveBeenCalled();
    expect((transport as unknown as { client: unknown }).client).toBeNull();
  });

  it("closes the local client when mailbox selection is interrupted", async () => {
    let finishOpen: (() => void) | undefined;
    const client = makeConnectClient({
      mailboxOpen: vi.fn(
        () =>
          new Promise<void>((resolve) => {
            finishOpen = resolve;
          }),
      ),
    });
    imapFlowMock.mockImplementationOnce(function ImapFlow() {
      return client;
    });
    const transport = new ImapTransport({ user: "u", pass: "p" }, logger);
    const connectEffect = (
      transport as unknown as { connectEffect: Effect.Effect<void, Error> }
    ).connectEffect;

    const fiber = runFork(connectEffect);
    await vi.waitFor(() => expect(client.mailboxOpen).toHaveBeenCalledOnce());
    await Effect.runPromise(Fiber.interrupt(fiber));
    expect(client.close).toHaveBeenCalledOnce();
    finishOpen?.();
    await Effect.runPromise(Effect.yieldNow);
    expect((transport as unknown as { client: unknown }).client).toBeNull();
  });
});

function makeConnectClient(
  overrides: Partial<{
    connect: () => Promise<void>;
    mailboxOpen: (folder: string, options: { readOnly: boolean }) => Promise<void>;
  }> = {},
) {
  return {
    on: vi.fn(),
    connect: vi.fn(async () => {}),
    mailboxOpen: vi.fn(async () => {}),
    close: vi.fn(),
    capabilities: new Set<string>(),
    ...overrides,
  };
}
