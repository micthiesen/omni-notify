import { Data, Effect } from "effect";

export class ParcelExtractionError extends Data.TaggedError("ParcelExtractionError")<{
  readonly cause: unknown;
  readonly transient: boolean;
}> {
  public override get message(): string {
    return `Parcel extraction failed: ${this.cause instanceof Error ? this.cause.message : String(this.cause)}`;
  }
}

export class CarrierListError extends Data.TaggedError("CarrierListError")<{
  readonly cause: unknown;
}> {
  public override get message(): string {
    return `Carrier list failed: ${this.cause instanceof Error ? this.cause.message : String(this.cause)}`;
  }
}

export class ParcelSubmissionError extends Data.TaggedError("ParcelSubmissionError")<{
  readonly cause: unknown;
}> {
  public override get message(): string {
    return `Parcel submission failed: ${this.cause instanceof Error ? this.cause.message : String(this.cause)}`;
  }
}

export class ParcelPersistenceError extends Data.TaggedError("ParcelPersistenceError")<{
  readonly operation: string;
  readonly cause: unknown;
}> {
  public readonly transient = true;

  public override get message(): string {
    const detail =
      this.cause instanceof Error ? this.cause.message : String(this.cause);
    return `${this.operation} failed: ${detail}`;
  }
}

export function parcelPersistenceEffect<A, E, R>(
  operation: string,
  evaluate: () => Effect.Effect<A, E, R>,
): Effect.Effect<A, ParcelPersistenceError, R> {
  return evaluate().pipe(
    Effect.mapError((cause) => new ParcelPersistenceError({ operation, cause })),
  );
}
