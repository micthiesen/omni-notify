import type { Logger } from "@micthiesen/mitools/logging";
import { Deferred, Effect, Fiber } from "effect";
import { beforeEach, describe, expect, it, vi } from "vitest";

const eventSourceState = vi.hoisted(() => ({
  instances: [] as Array<{
    addEventListener: ReturnType<typeof vi.fn>;
    close: ReturnType<typeof vi.fn>;
    emit: (event: string, payload?: unknown) => void;
  }>,
  throwOnEvent: undefined as string | undefined,
  throwOnEventRemaining: 0,
}));

vi.mock("eventsource", () => ({
  EventSource: vi.fn(function EventSource() {
    const listeners = new Map<string, Array<(event: unknown) => void>>();
    const instance = {
      addEventListener: vi.fn((event: string, listener: (event: unknown) => void) => {
        if (
          eventSourceState.throwOnEvent === event &&
          eventSourceState.throwOnEventRemaining > 0
        ) {
          eventSourceState.throwOnEventRemaining -= 1;
          throw new Error(`cannot register ${event}`);
        }
        listeners.set(event, [...(listeners.get(event) ?? []), listener]);
      }),
      close: vi.fn(),
      emit: (event: string, payload: unknown = {}) => {
        for (const listener of listeners.get(event) ?? []) listener(payload);
      },
    };
    eventSourceState.instances.push(instance);
    return instance;
  }),
}));

import type { JmapContext } from "./client.js";
import { createEventSourceEffect } from "./eventSource.js";

const logger = {
  debug: vi.fn(),
  info: vi.fn(),
  warn: vi.fn(),
  error: vi.fn(),
  extend: vi.fn(),
} as unknown as Logger;

function context(
  session: Promise<{ eventSourceUrl: string }> = Promise.resolve({
    eventSourceUrl: "https://example.test/events",
  }),
): JmapContext {
  return {
    jam: { session, authHeader: "Bearer secret" },
    accountId: "account",
  } as unknown as JmapContext;
}

describe("JMAP EventSource resource ownership", () => {
  beforeEach(() => {
    eventSourceState.instances.length = 0;
    eventSourceState.throwOnEvent = undefined;
    eventSourceState.throwOnEventRemaining = 0;
    vi.clearAllMocks();
  });

  it("closes the connection and timer fibers when startup ownership is interrupted", async () => {
    const keepAlive = await Effect.runPromise(Deferred.make<void>());
    const program = Effect.scoped(
      createEventSourceEffect(context(), vi.fn(), logger).pipe(
        Effect.andThen(Deferred.await(keepAlive)),
      ),
    );
    const fiber = Effect.runFork(program);
    await vi.waitFor(() => expect(eventSourceState.instances).toHaveLength(1));
    const instance = eventSourceState.instances[0];
    expect(instance.close).not.toHaveBeenCalled();

    await Effect.runPromise(Fiber.interrupt(fiber));
    expect(instance.close).toHaveBeenCalledOnce();
  });

  it("closes a partially initialized EventSource when listener setup fails", async () => {
    eventSourceState.throwOnEvent = "state";
    eventSourceState.throwOnEventRemaining = 1;

    await expect(
      Effect.runPromise(
        Effect.scoped(createEventSourceEffect(context(), vi.fn(), logger)),
      ),
    ).rejects.toThrow("cannot register state");
    expect(eventSourceState.instances).toHaveLength(1);
    expect(eventSourceState.instances[0].close).toHaveBeenCalledOnce();
  });

  it("does not allocate an EventSource when session discovery is interrupted", async () => {
    const neverSession = new Promise<{ eventSourceUrl: string }>(() => {});
    const fiber = Effect.runFork(
      Effect.scoped(createEventSourceEffect(context(neverSession), vi.fn(), logger)),
    );
    await Effect.runPromise(Effect.yieldNow);

    await Effect.runPromise(Fiber.interrupt(fiber));
    expect(eventSourceState.instances).toHaveLength(0);
  });

  it("keeps reconnecting after transient listener setup failure", async () => {
    vi.useFakeTimers();
    try {
      const keepAlive = await Effect.runPromise(Deferred.make<void>());
      const onEmailStateChange = vi.fn();
      const fiber = Effect.runFork(
        Effect.scoped(
          createEventSourceEffect(context(), onEmailStateChange, logger).pipe(
            Effect.andThen(Deferred.await(keepAlive)),
          ),
        ),
      );
      await Promise.resolve();
      await Promise.resolve();
      expect(eventSourceState.instances).toHaveLength(1);

      eventSourceState.throwOnEvent = "state";
      eventSourceState.throwOnEventRemaining = 1;
      await vi.advanceTimersByTimeAsync(6 * 60_000 + 20_000);

      expect(eventSourceState.instances).toHaveLength(3);
      expect(eventSourceState.instances[1].close).toHaveBeenCalledOnce();
      eventSourceState.instances[2].emit("open");
      eventSourceState.instances[2].emit("state", {
        data: JSON.stringify({ changed: { account: { Email: "next-state" } } }),
      });
      expect(onEmailStateChange).toHaveBeenCalledTimes(2);

      await Effect.runPromise(Fiber.interrupt(fiber));
      expect(eventSourceState.instances[2].close).toHaveBeenCalledOnce();
    } finally {
      vi.useRealTimers();
    }
  });
});
