import type { Logger } from "@micthiesen/mitools/logging";
import { Deferred, Effect, Fiber } from "effect";
import { describe, expect, it, vi } from "vitest";

const imapFlowMock = vi.hoisted(() => vi.fn());
vi.mock("imapflow", () => ({ ImapFlow: imapFlowMock }));

import { ImapTransport } from "./transport.js";

const logger = {
  debug: vi.fn(),
  info: vi.fn(),
  warn: vi.fn(),
  error: vi.fn(),
  extend: vi.fn(),
} as unknown as Logger;

describe("ImapTransport mailbox serialization", () => {
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
    const fiber = Effect.runFork(searches);
    await Effect.runPromise(Deferred.await(firstLockRequested));
    await Effect.runPromise(Deferred.succeed(releaseFirstLock, undefined));
    await Effect.runPromise(Fiber.join(fiber));

    expect(maxActive).toBe(1);
    expect(mailbox.path).toBe("INBOX");
    expect(client.mailboxOpen).toHaveBeenCalledTimes(2);
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
    const stop = Effect.runPromise(transport.stopEffect);
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

    await expect(Effect.runPromise(connectEffect)).rejects.toThrow("select failed");
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

    await expect(Effect.runPromise(connectEffect)).rejects.toThrow("connect failed");
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

    const fiber = Effect.runFork(connectEffect);
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

    const fiber = Effect.runFork(connectEffect);
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
