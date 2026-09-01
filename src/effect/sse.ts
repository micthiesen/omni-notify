import { Effect, Fiber } from "effect";

/** Keep an SSE scope alive until normal completion/disconnect, but propagate a
 * writer failure immediately so the surrounding scope releases subscriptions. */
export function awaitSseWriter<E, E2>(
  writer: Fiber.RuntimeFiber<never, E>,
  completion: Effect.Effect<void, E2>,
): Effect.Effect<void, E | E2> {
  return Effect.raceFirst(Fiber.join(writer), completion);
}
