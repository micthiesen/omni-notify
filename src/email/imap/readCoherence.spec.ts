import { describe, expect, it } from "@effect/vitest";
import { Logger } from "@micthiesen/mitools/logging";
import { Deferred, Effect, Fiber } from "effect";
import { TestClock } from "effect/testing";
import { vi } from "vitest";
import { ImapTransport } from "./transport.js";

function fixture() {
  let ids = [1];
  const mailbox = { path: "INBOX", uidValidity: 1 };
  const source = (uid: number) =>
    Buffer.from(
      `Message-ID: <${uid}@example.test>\r\nSubject: message-${uid}\r\n\r\nBody`,
    );
  const client = {
    usable: true,
    list: vi.fn(async () => []),
    mailbox,
    getMailboxLock: vi.fn(async (path: string) => {
      mailbox.path = path;
      return { release() {} };
    }),
    mailboxOpen: vi.fn(async (path: string) => {
      mailbox.path = path;
    }),
    search: vi.fn(async () => ids),
    fetch: vi.fn(async function* (uids: number[]) {
      for (const uid of uids)
        yield {
          uid,
          source: source(uid),
          internalDate: new Date("2026-09-30T12:00:00Z"),
        };
    }),
    fetchOne: vi.fn(async (uid: string) => ({
      uid: Number(uid),
      source: source(Number(uid)),
    })),
  };
  const logger = Logger.named("ImapReadCacheSpec");
  const transport = new ImapTransport(
    { user: "me@example.test", pass: "fake" },
    logger,
  );
  Object.assign(transport, { client });
  return {
    transport,
    client,
    setIds: (value: number[]) => {
      ids = value;
    },
  };
}

describe("IMAP read cache coherence", () => {
  it.effect(
    "expires search snapshots on the Effect clock while retaining immutable parsed bodies",
    () =>
      Effect.gen(function* () {
        const { transport, client } = fixture();
        const options = { folder: "inbox" as const, limit: 1 };
        yield* transport.searchEmailsEffect(options);
        yield* transport.searchEmailsEffect(options);
        expect(client.search).toHaveBeenCalledOnce();
        yield* TestClock.adjust(30_001);
        yield* transport.searchEmailsEffect(options);
        expect(client.search).toHaveBeenCalledTimes(2);
        expect(client.fetch).toHaveBeenCalledOnce();
        yield* TestClock.adjust(300_001);
        yield* transport.searchEmailsEffect(options);
        expect(client.fetch).toHaveBeenCalledTimes(2);
      }).pipe(Effect.provide(Logger.layer())),
  );

  it.effect(
    "replaces a stale search snapshot after a fresh read and separates UIDVALIDITY generations",
    () =>
      Effect.gen(function* () {
        const { transport, client, setIds } = fixture();
        const options = { folder: "inbox" as const, limit: 1 };
        yield* transport.searchEmailsEffect(options);
        setIds([2]);
        expect((yield* transport.searchEmailsEffect(options))[0]?.subject).toBe(
          "message-1",
        );
        yield* transport.searchEmailsEffect({ ...options, fresh: true });
        expect((yield* transport.searchEmailsEffect(options))[0]?.subject).toBe(
          "message-2",
        );
        yield* TestClock.adjust(30_001);
        client.mailbox.uidValidity = 2;
        yield* transport.searchEmailsEffect(options);
        expect(client.fetch).toHaveBeenCalledTimes(3);
      }).pipe(Effect.provide(Logger.layer())),
  );

  it.effect(
    "serves cached reads while background processing owns the mailbox permit",
    () =>
      Effect.gen(function* () {
        const { transport } = fixture();
        const options = { folder: "inbox" as const, limit: 1 };
        yield* transport.searchEmailsEffect(options);
        yield* transport.fetchEmailByIdEffect("<1@example.test>");
        const entered = yield* Deferred.make<void>();
        const release = yield* Deferred.make<void>();
        const internal = transport as unknown as {
          runSerializedEffect: (
            name: string,
            effect: Effect.Effect<void>,
          ) => Effect.Effect<void>;
        };
        const background = yield* internal
          .runSerializedEffect(
            "background",
            Effect.gen(function* () {
              yield* Deferred.succeed(entered, undefined);
              yield* Deferred.await(release);
            }),
          )
          .pipe(Effect.forkChild);
        yield* Deferred.await(entered);
        expect(yield* transport.searchEmailsEffect(options)).toHaveLength(1);
        expect(
          (yield* transport.fetchEmailByIdEffect("<1@example.test>"))?.subject,
        ).toBe("message-1");
        yield* Deferred.succeed(release, undefined);
        yield* Fiber.join(background);
      }).pipe(Effect.provide(Logger.layer())),
  );
});
