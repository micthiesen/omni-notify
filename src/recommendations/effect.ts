import { Data, Effect } from "effect";

/** Typed failure for recommendation integrations and model calls. */
export class RecommendationIntegrationError extends Data.TaggedError(
  "RecommendationIntegrationError",
)<{
  readonly operation: string;
  readonly cause: unknown;
}> {
  public override get message(): string {
    const detail =
      this.cause instanceof Error ? this.cause.message : String(this.cause);
    return `${this.operation} failed: ${detail}`;
  }
}

/** Typed failure for an invalid recommendation pipeline request. */
export class RecommendationInputError extends Data.TaggedError(
  "RecommendationInputError",
)<{ readonly message: string }> {}

export class RecommendationPersistenceError extends Data.TaggedError(
  "RecommendationPersistenceError",
)<{ readonly operation: string; readonly cause: unknown }> {}

export class RecommendationCommitError extends Data.TaggedError(
  "RecommendationCommitError",
)<{ readonly message: string }> {}

export class TasteReflectionOutputError extends Data.TaggedError(
  "TasteReflectionOutputError",
)<{ readonly message: string }> {}

export function integrationEffect<A>(
  operation: string,
  evaluate: (signal: AbortSignal) => A | PromiseLike<A>,
): Effect.Effect<A, RecommendationIntegrationError> {
  return Effect.tryPromise({
    try: (signal) => Promise.resolve(evaluate(signal)),
    catch: (cause) => new RecommendationIntegrationError({ operation, cause }),
  });
}

export function persistenceEffect<A, E, R>(
  operation: string,
  evaluate: () => Effect.Effect<A, E, R>,
): Effect.Effect<A, RecommendationPersistenceError, R> {
  return Effect.suspend(evaluate).pipe(
    Effect.mapError(
      (cause) => new RecommendationPersistenceError({ operation, cause }),
    ),
  );
}

export function effectMessage(cause: unknown): string {
  if (cause instanceof RecommendationIntegrationError) {
    return effectMessage(cause.cause);
  }
  return cause instanceof Error ? cause.message : String(cause);
}
