import { describe, expect, it } from "@effect/vitest";
import { Deferred, Effect } from "effect";
import { makeUiCallbackRuntime } from "./effect";

describe("makeUiCallbackRuntime", () => {
  it.effect("interrupts in-flight browser callback work when its scope closes", () =>
    Effect.gen(function* () {
      const started = yield* Deferred.make<void>();
      let finalized = false;
      let updatedAfterCleanup = false;

      yield* Effect.scoped(
        Effect.gen(function* () {
          const runCallback = yield* makeUiCallbackRuntime();
          runCallback(
            Deferred.succeed(started, undefined).pipe(
              Effect.andThen(Effect.never),
              Effect.tap(() =>
                Effect.sync(() => {
                  updatedAfterCleanup = true;
                }),
              ),
              Effect.ensuring(
                Effect.sync(() => {
                  finalized = true;
                }),
              ),
            ),
          );
          yield* Deferred.await(started);
        }),
      );

      expect(finalized).toBe(true);
      expect(updatedAfterCleanup).toBe(false);
    }),
  );
});
