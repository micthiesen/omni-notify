import { Data } from "effect";

/** A failure raised while adapting a non-Effect API at an infrastructure edge. */
export class IntegrationError extends Data.TaggedError("IntegrationError")<{
  readonly operation: string;
  readonly cause: unknown;
}> {
  public override get message(): string {
    const detail =
      this.cause instanceof Error ? this.cause.message : String(this.cause);
    return `${this.operation} failed: ${detail}`;
  }
}

/** A failure raised while adapting synchronous persistence code. */
export class PersistenceError extends Data.TaggedError("PersistenceError")<{
  readonly operation: string;
  readonly cause: unknown;
}> {
  public override get message(): string {
    const detail =
      this.cause instanceof Error ? this.cause.message : String(this.cause);
    return `${this.operation} failed: ${detail}`;
  }
}
