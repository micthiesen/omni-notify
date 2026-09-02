import { Deferred, Effect, Fiber, Semaphore } from "effect";
import { describe, expect, it } from "vitest";
import { awaitSseWriter, enqueueInitialSnapshotFrame } from "./sse.js";

describe("enqueueInitialSnapshotFrame", () => {
  it("prevents a newer broadcast from being overwritten by the initial frame", async () => {
    const frames = await Effect.runPromise(
      Effect.gen(function* () {
        const semaphore = Semaphore.makeUnsafe(1);
        const initialStarted = yield* Deferred.make<void>();
        const releaseInitial = yield* Deferred.make<void>();
        const frames: Array<{ event: "snapshot"; data: string; id: string }> = [];
        let liveChannel = "first";
        let registered = false;
        let eventId = 0;
        const initial = yield* enqueueInitialSnapshotFrame(
          semaphore,
          () => {
            const snapshot = { liveChannel };
            return Deferred.succeed(initialStarted, undefined).pipe(
              Effect.andThen(Deferred.await(releaseInitial)),
              Effect.as(snapshot),
            );
          },
          () => String(eventId++),
          (frame) => {
            registered = true;
            frames.push(frame);
          },
        ).pipe(Effect.forkChild);

        yield* Deferred.await(initialStarted);
        liveChannel = "second";
        const broadcast = yield* semaphore
          .withPermits(1)(
            Effect.sync(() => {
              if (registered) {
                frames.push({
                  event: "snapshot",
                  data: JSON.stringify({ liveChannel }),
                  id: String(eventId++),
                });
              }
            }),
          )
          .pipe(Effect.forkChild);
        yield* Deferred.succeed(releaseInitial, undefined);
        yield* Fiber.join(initial);
        yield* Fiber.join(broadcast);
        return frames;
      }),
    );

    expect(frames).toEqual([
      {
        event: "snapshot",
        data: JSON.stringify({ liveChannel: "first" }),
        id: "0",
      },
      {
        event: "snapshot",
        data: JSON.stringify({ liveChannel: "second" }),
        id: "1",
      },
    ]);
  });
});

describe("awaitSseWriter", () => {
  it("propagates writer failure and releases the parent SSE scope", async () => {
    let released = false;
    const program = Effect.scoped(
      Effect.gen(function* () {
        yield* Effect.addFinalizer(() =>
          Effect.sync(() => {
            released = true;
          }),
        );
        const writer = yield* Effect.fail(new Error("socket write failed")).pipe(
          Effect.andThen(Effect.never),
          Effect.forkScoped,
        );
        yield* awaitSseWriter(writer, Effect.never);
      }),
    );
    await expect(Effect.runPromise(program)).rejects.toThrow("socket write failed");
    expect(released).toBe(true);
  });
});
