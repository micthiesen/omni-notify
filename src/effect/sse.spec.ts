import { Effect } from "effect";
import { describe, expect, it } from "vitest";
import { awaitSseWriter } from "./sse.js";

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
