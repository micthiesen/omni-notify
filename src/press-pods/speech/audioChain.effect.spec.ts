import { execFile } from "node:child_process";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { promisify } from "node:util";
import { describe, expect, it } from "@effect/vitest";
import { Duration, Effect, Exit, Fiber, TestClock } from "effect";
import { assembleEpisode } from "./audioChain.js";

const execFileAsync = promisify(execFile);

async function pressPodsTemporaryFiles(): Promise<Set<string>> {
  return new Set(
    (await fs.readdir(os.tmpdir())).filter((name) => name.startsWith("pp_")),
  );
}

describe("audioChain resource cleanup", () => {
  it.effect("kills an interrupted ffmpeg child and removes every owned temp file", () =>
    Effect.gen(function* () {
      const fifo = path.join(
        os.tmpdir(),
        `presspods-audiochain-${process.pid}-${Date.now()}.wav`,
      );
      yield* Effect.tryPromise(() => execFileAsync("mkfifo", [fifo]));
      const before = yield* Effect.tryPromise(pressPodsTemporaryFiles);

      const fiber = yield* Effect.fork(
        assembleEpisode([fifo], Buffer.from("unused intro")).pipe(
          Effect.timeout(Duration.millis(50)),
        ),
      );
      yield* Effect.yieldNow();
      yield* TestClock.adjust(Duration.millis(50));
      const exit = yield* Fiber.await(fiber);
      expect(Exit.isFailure(exit)).toBe(true);
      yield* Effect.tryPromise(() => fs.rm(fifo, { force: true }));

      const after = yield* Effect.tryPromise(pressPodsTemporaryFiles);
      expect([...after].filter((file) => !before.has(file))).toEqual([]);
    }),
  );
});
