import { Effect } from "effect";
import { WorkspaceOperationError } from "./errors.js";

/** Typed leaf adapter for the synchronous SQLite entity repository. */
export function workspaceRepositoryEffect<A>(
  operation: string,
  evaluate: () => A,
): Effect.Effect<A, WorkspaceOperationError> {
  return Effect.try({
    try: evaluate,
    catch: (cause) => new WorkspaceOperationError({ operation, cause }),
  });
}
