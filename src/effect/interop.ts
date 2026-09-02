import { Effect } from "effect";
import { IntegrationError, PersistenceError } from "./errors.js";

/**
 * Adapt one Promise-returning library call into a typed Effect. The AbortSignal
 * is passed through when the underlying API supports cancellation.
 */
export function fromPromise<A>(
  operation: string,
  evaluate: (signal: AbortSignal) => PromiseLike<A>,
): Effect.Effect<A, IntegrationError> {
  return Effect.tryPromise({
    try: evaluate,
    catch: (cause) => new IntegrationError({ operation, cause }),
  });
}

/** Adapt one throwing persistence/library call into a typed Effect. */
export function fromSync<A>(
  operation: string,
  evaluate: () => A,
): Effect.Effect<A, PersistenceError> {
  return Effect.try({
    try: evaluate,
    catch: (cause) => new PersistenceError({ operation, cause }),
  });
}

/**
 * Interpret an Effect only at a framework boundary that still requires a
 * Promise, such as Hono, MCP, or mitools ScheduledTask.
 */
export function runPromise<A, E>(effect: Effect.Effect<A, E, never>): Promise<A> {
  return Effect.runPromise(effect);
}
