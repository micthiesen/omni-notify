import type { Logger } from "@micthiesen/mitools/logging";
import { Deferred, Effect } from "effect";
import { runPromise } from "../effect/interop.js";
import { describe, expect, it, vi } from "vitest";

vi.mock("./persistence.js", () => ({
  saveLastDispatchedAtEffect: vi.fn(() => Effect.void),
}));

import { EmailDispatcher } from "./dispatcher.js";
import type { EmailHandler, EmailPoll, EmailTransport, FetchedEmail } from "./types.js";

const email: FetchedEmail = {
  id: "email-1",
  subject: "Shipment",
  from: "shop@example.com",
  textBody: "On its way",
  links: [],
  receivedAt: new Date(0).toISOString(),
  attachments: [],
};

const logger = {
  debug: vi.fn(),
  info: vi.fn(),
  warn: vi.fn(),
  error: vi.fn(),
  extend: vi.fn(),
} as unknown as Logger;

function transport(poll: Effect.Effect<EmailPoll>): EmailTransport {
  return {
    name: "test",
    startEffect: () => Effect.void,
    stopEffect: Effect.void,
    pollNewEmailsEffect: poll,
    fetchEmailByIdEffect: () => Effect.succeed(undefined),
    downloadAttachmentEffect: () => Effect.succeed(undefined),
  };
}

describe("EmailDispatcher durability", () => {
  it("does not commit the cursor when any handler fails", async () => {
    const commit = vi.fn();
    const dispatcher = new EmailDispatcher(
      transport(Effect.succeed({ emails: [email], commit })),
      logger,
    );
    const successful: EmailHandler = {
      name: "successful",
      handleEmailsEffect: vi.fn(() => Effect.void),
    };
    const failed: EmailHandler = {
      name: "failed",
      handleEmailsEffect: vi.fn(() => Effect.fail(new Error("not durable"))),
    };
    dispatcher.register(successful);
    dispatcher.register(failed);

    await runPromise(dispatcher.onMailEventEffect);

    expect(failed.handleEmailsEffect).toHaveBeenCalledOnce();
    expect(logger.error).toHaveBeenCalled();
    expect(successful.handleEmailsEffect).toHaveBeenCalledOnce();
    expect(commit).not.toHaveBeenCalled();
  });

  it("commits only after every handler succeeds", async () => {
    const commit = vi.fn();
    const dispatcher = new EmailDispatcher(
      transport(Effect.succeed({ emails: [email], commit })),
      logger,
    );
    dispatcher.register({ name: "ok", handleEmailsEffect: vi.fn(() => Effect.void) });

    await runPromise(dispatcher.onMailEventEffect);
    expect(commit).toHaveBeenCalledOnce();
  });

  it("drains a notification that arrives while the active pass is completing", async () => {
    const firstStarted = await runPromise(Deferred.make<void>());
    const releaseFirst = await runPromise(Deferred.make<void>());
    const secondFinished = await runPromise(Deferred.make<void>());
    let pollCount = 0;
    let notify: (() => void) | undefined;
    const lifecycleTransport: EmailTransport = {
      ...transport(
        Effect.suspend(() => {
          pollCount++;
          if (pollCount === 1) {
            return Deferred.succeed(firstStarted, undefined).pipe(
              Effect.andThen(Deferred.await(releaseFirst)),
              Effect.as({ emails: [], commit: vi.fn() }),
            );
          }
          return Deferred.succeed(secondFinished, undefined).pipe(
            Effect.as({ emails: [], commit: vi.fn() }),
          );
        }),
      ),
      startEffect: (onMailEvent) =>
        Effect.sync(() => {
          notify = onMailEvent;
        }),
    };
    const dispatcher = new EmailDispatcher(lifecycleTransport, logger);

    await runPromise(dispatcher.startEffect);
    notify?.();
    await runPromise(Deferred.await(firstStarted));
    notify?.();
    notify?.();
    notify?.();

    await runPromise(Deferred.succeed(releaseFirst, undefined));
    await runPromise(dispatcher.stopEffect);
    await runPromise(Deferred.await(secondFinished));
    expect(pollCount).toBe(2);

    notify?.();
    await runPromise(Effect.yieldNow);
    expect(pollCount).toBe(2);
  });

  it("stops the transport and supervisor when startup fails", async () => {
    const stopped = vi.fn();
    let pollCount = 0;
    const dispatcher = new EmailDispatcher(
      {
        ...transport(
          Effect.sync(() => {
            pollCount++;
            return { emails: [], commit: vi.fn() };
          }),
        ),
        startEffect: () => Effect.fail(new Error("connection failed")),
        stopEffect: Effect.sync(stopped),
      },
      logger,
    );

    await expect(runPromise(dispatcher.startEffect)).rejects.toThrow(
      "connection failed",
    );
    expect(stopped).toHaveBeenCalledOnce();

    dispatcher.onMailEvent();
    await runPromise(Effect.yieldNow);
    expect(pollCount).toBe(0);
  });
});
