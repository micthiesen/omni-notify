import { Data, Effect } from "effect";

export class PressPodsError extends Data.TaggedError("PressPodsError")<{
  readonly operation: string;
  readonly cause: unknown;
  readonly retryable?: boolean;
}> {
  public override get message(): string {
    const detail =
      this.cause instanceof Error ? this.cause.message : String(this.cause);
    return `${this.operation}: ${detail}`;
  }
}

export class InvalidPressPodsDataError extends Data.TaggedError(
  "InvalidPressPodsDataError",
)<{
  readonly operation: string;
  readonly cause: unknown;
}> {
  public override get message(): string {
    const detail =
      this.cause instanceof Error ? this.cause.message : String(this.cause);
    return `${this.operation}: ${detail}`;
  }
}

export const trySync = <A>(operation: string, evaluate: () => A) =>
  Effect.try({
    try: evaluate,
    catch: (cause) => new PressPodsError({ operation, cause }),
  });

export const tryPromise = <A>(
  operation: string,
  evaluate: (signal: AbortSignal) => PromiseLike<A>,
) =>
  Effect.tryPromise({
    try: evaluate,
    catch: (cause) => new PressPodsError({ operation, cause }),
  });

export const ignoreFailure = <A, E>(effect: Effect.Effect<A, E>): Effect.Effect<void> =>
  effect.pipe(
    Effect.asVoid,
    Effect.catchAll(() => Effect.void),
  );

export const errorCause = (error: unknown): unknown =>
  error instanceof PressPodsError || error instanceof InvalidPressPodsDataError
    ? error.cause
    : error;
