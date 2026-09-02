import { describe, expect, it } from "@effect/vitest";
import { Effect, Fiber } from "effect";
import { TestClock } from "effect/testing";
import { callLanguageModelEffect, LANGUAGE_MODEL_TIMEOUT } from "./registry.js";

describe("callLanguageModelEffect", () => {
  it.effect("times out and aborts a hanging model request", () =>
    Effect.gen(function* () {
      let signal: AbortSignal | undefined;
      const fiber = yield* Effect.forkChild(
        callLanguageModelEffect(
          (requestSignal) =>
            new Promise<never>((_resolve, reject) => {
              signal = requestSignal;
              requestSignal.addEventListener(
                "abort",
                () => reject(new DOMException("aborted", "AbortError")),
                { once: true },
              );
            }),
        ),
      );

      yield* Effect.yieldNow;
      yield* TestClock.adjust(LANGUAGE_MODEL_TIMEOUT);
      const result = yield* Fiber.await(fiber);

      expect(result._tag).toBe("Failure");
      expect(signal?.aborted).toBe(true);
    }),
  );
});
