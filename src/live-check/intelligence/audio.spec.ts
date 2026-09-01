import { describe, expect, it } from "@effect/vitest";
import { Cause, Effect, Exit, Fiber } from "effect";
import { TestClock } from "effect/testing";
import { AudioProcessError, runAudioProcess } from "./audio.js";

describe("runAudioProcess", () => {
  it.effect("classifies a deterministic output limit as non-retryable", () =>
    Effect.gen(function* () {
      const exit = yield* Effect.exit(
        runAudioProcess(process.execPath, ["-e", "process.stdout.write('overflow')"], {
          timeoutMs: 2_000,
          maxStdoutBytes: 2,
        }),
      );
      expect(Exit.isFailure(exit)).toBe(true);
      if (Exit.isFailure(exit)) {
        const reason = exit.cause.reasons.find(Cause.isFailReason);
        const failure = reason?.error;
        expect(failure).toBeInstanceOf(AudioProcessError);
        expect(failure?.retryable).toBe(false);
        expect(failure?.message).toContain("exceeded output limit");
      }
    }),
  );

  it.effect(
    "classifies a deterministic timeout as non-retryable and interrupts the child",
    () =>
      Effect.gen(function* () {
        const fiber = yield* Effect.forkChild(
          Effect.exit(
            runAudioProcess(process.execPath, ["-e", "setInterval(() => {}, 1000)"], {
              timeoutMs: 25,
              maxStdoutBytes: 100,
            }),
          ),
        );
        yield* TestClock.adjust("25 millis");
        const exit = yield* Fiber.join(fiber);
        expect(Exit.isFailure(exit)).toBe(true);
        if (Exit.isFailure(exit)) {
          const reason = exit.cause.reasons.find(Cause.isFailReason);
          const failure = reason?.error;
          expect(failure).toBeInstanceOf(AudioProcessError);
          expect(failure?.retryable).toBe(false);
          expect(failure?.message).toContain("timed out after 25ms");
        }
      }),
  );
});
