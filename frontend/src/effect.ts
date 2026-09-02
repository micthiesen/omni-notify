import { Effect, FiberSet } from "effect";
export { useEffectFiber as useUiEffect } from "@micthiesen/mitools/react";

/** Interpret an application Effect at a React or browser callback boundary. */
export const runUiEffect = <A, E>(effect: Effect.Effect<A, E>): Promise<A> =>
  Effect.runPromise(effect);

/** Start a UI lifecycle Effect and return an idempotent React cleanup callback. */
export const forkUiEffect = <A, E>(effect: Effect.Effect<A, E>): (() => void) => {
  const fiber = Effect.runFork(effect);
  return () => {
    fiber.interruptUnsafe();
  };
};

/**
 * Capture a callback runtime whose fibers belong to the current Scope.
 * Browser callbacks are synchronous, so this is the bridge that lets them
 * start Effects without allowing work to outlive the connection/component.
 */
export const makeUiCallbackRuntime = FiberSet.makeRuntime<never, unknown, never>;

/** Run an Effect for a mounted React lifecycle without creating a Promise chain. */
export const forkUiRequest = <A, E>(
  effect: Effect.Effect<A, E>,
  handlers: {
    readonly onSuccess: (value: A) => void;
    readonly onFailure?: (error: E) => void;
  },
): (() => void) =>
  forkUiEffect(
    effect.pipe(
      Effect.match({
        onFailure: (error) => handlers.onFailure?.(error),
        onSuccess: handlers.onSuccess,
      }),
    ),
  );
