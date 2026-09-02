import { Effect, Fiber, Semaphore } from "effect";

export interface SseSnapshotFrame {
  readonly event: "snapshot";
  readonly data: string;
  readonly id: string;
}

/** Build and enqueue a new client's current snapshot without letting a shared
 * broadcast overtake it. The callback should register the client and enqueue
 * the frame in the same synchronous operation. */
export function enqueueInitialSnapshotFrame<A, E, R>(
  semaphore: Semaphore.Semaphore,
  buildSnapshot: () => Effect.Effect<A, E, R>,
  nextId: () => string,
  enqueue: (frame: SseSnapshotFrame) => void,
): Effect.Effect<void, E, R> {
  return semaphore.withPermits(1)(
    Effect.suspend(buildSnapshot).pipe(
      Effect.map((snapshot) => ({
        event: "snapshot" as const,
        data: JSON.stringify(snapshot),
        id: nextId(),
      })),
      Effect.tap((frame) => Effect.sync(() => enqueue(frame))),
      Effect.asVoid,
    ),
  );
}

/** Keep an SSE scope alive until normal completion/disconnect, but propagate a
 * writer failure immediately so the surrounding scope releases subscriptions. */
export function awaitSseWriter<E, E2>(
  writer: Fiber.Fiber<never, E>,
  completion: Effect.Effect<void, E2>,
): Effect.Effect<void, E | E2> {
  return Effect.raceFirst(Fiber.join(writer), completion).pipe(Effect.asVoid);
}
