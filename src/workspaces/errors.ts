import { Data } from "effect";

export class WorkspaceValidationError extends Data.TaggedError(
  "WorkspaceValidationError",
)<{ readonly message: string; readonly cause?: unknown }> {}

export class WorkspaceOperationError extends Data.TaggedError(
  "WorkspaceOperationError",
)<{ readonly operation: string; readonly cause: unknown }> {
  public override get message(): string {
    const detail =
      this.cause instanceof Error ? this.cause.message : String(this.cause);
    return `${this.operation} failed: ${detail}`;
  }
}

export class WorkspaceActionError extends Data.TaggedError("WorkspaceActionError")<{
  readonly actionId: string;
  readonly message: string;
  readonly cause?: unknown;
}> {}
