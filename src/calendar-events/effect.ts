import { Data, Effect } from "effect";

export class CalendarExtractionError extends Data.TaggedError(
  "CalendarExtractionError",
)<{
  readonly cause: unknown;
  readonly transient: boolean;
}> {
  public override get message(): string {
    return `Calendar extraction failed: ${this.cause instanceof Error ? this.cause.message : String(this.cause)}`;
  }
}

export class CaldavError extends Data.TaggedError("CaldavError")<{
  readonly operation: string;
  readonly cause: unknown;
  readonly transient: boolean;
  readonly statusCode?: number;
}> {
  public override get message(): string {
    return `${this.operation} failed: ${this.cause instanceof Error ? this.cause.message : String(this.cause)}`;
  }
}

export class AttachmentDownloadError extends Data.TaggedError(
  "AttachmentDownloadError",
)<{
  readonly name: string;
  readonly cause: unknown;
}> {}

export class CalendarPersistenceError extends Data.TaggedError(
  "CalendarPersistenceError",
)<{ readonly operation: string; readonly cause: unknown }> {
  public override get message(): string {
    const detail =
      this.cause instanceof Error ? this.cause.message : String(this.cause);
    return `${this.operation} failed: ${detail}`;
  }
}

export function calendarPersistenceEffect<A>(
  operation: string,
  evaluate: () => A,
): Effect.Effect<A, CalendarPersistenceError> {
  return Effect.try({
    try: evaluate,
    catch: (cause) => new CalendarPersistenceError({ operation, cause }),
  });
}
