import { Effect } from "effect";
import { WorkspaceOperationError } from "./errors.js";

/** Typed leaf adapter for the synchronous SQLite entity repository. */
export function workspaceRepositoryEffect<A, E, R>(
  operation: string,
  evaluate: () => Effect.Effect<A, E, R>,
): Effect.Effect<A, WorkspaceOperationError, R> {
  return evaluate().pipe(
    Effect.mapError((cause) => new WorkspaceOperationError({ operation, cause })),
  );
}
