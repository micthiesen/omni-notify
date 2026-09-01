import { it } from "@effect/vitest";
import { Deferred, Effect, Fiber } from "effect";
import { describe, expect } from "vitest";
import { loadTasteSeedEffect, TasteSeedError } from "./taste.js";

describe("loadTasteSeedEffect", () => {
  it.effect("returns a typed failure when the seed cannot be read", () =>
    loadTasteSeedEffect("/definitely/missing/podcast-taste.md").pipe(
      Effect.flip,
      Effect.tap((error) =>
        Effect.sync(() => {
          expect(error).toBeInstanceOf(TasteSeedError);
          expect(error.reason).toBe("unreadable");
          expect(error.path).toBe("/definitely/missing/podcast-taste.md");
        }),
      ),
      Effect.asVoid,
    ),
  );

  it.effect("rejects an empty seed as malformed", () =>
    loadTasteSeedEffect("taste.md", () =>
      Effect.runPromise(Effect.succeed(" \n\t ")),
    ).pipe(
      Effect.flip,
      Effect.tap((error) =>
        Effect.sync(() => {
          expect(error).toBeInstanceOf(TasteSeedError);
          expect(error.reason).toBe("malformed");
          expect(error.message).toContain("is empty");
        }),
      ),
      Effect.asVoid,
    ),
  );

  it.effect("aborts the in-flight read when interrupted", () =>
    Effect.gen(function* () {
      let observedSignal: AbortSignal | undefined;
      const started = yield* Deferred.make<void>();
      const read = (_path: string, signal: AbortSignal): Promise<string> => {
        observedSignal = signal;
        Effect.runSync(Deferred.succeed(started, undefined));
        return Effect.runPromise(Effect.never);
      };

      const fiber = yield* Effect.forkChild(loadTasteSeedEffect("taste.md", read));
      yield* Deferred.await(started);
      yield* Fiber.interrupt(fiber);

      expect(observedSignal?.aborted).toBe(true);
    }),
  );
});
