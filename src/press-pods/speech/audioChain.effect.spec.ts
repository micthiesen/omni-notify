import { execFile } from "node:child_process";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { promisify } from "node:util";
import { describe, expect, it } from "@effect/vitest";
import { withVirtualTime } from "@micthiesen/mitools/testing";
import { Effect, Exit } from "effect";
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

      const exit = yield* Effect.exit(
        withVirtualTime(
          assembleEpisode([fifo], Buffer.from("unused intro")).pipe(Effect.timeout(50)),
          50,
        ),
      );
      expect(Exit.isFailure(exit)).toBe(true);
      yield* Effect.tryPromise(() => fs.rm(fifo, { force: true }));

      const after = yield* Effect.tryPromise(pressPodsTemporaryFiles);
      expect([...after].filter((file) => !before.has(file))).toEqual([]);
    }),
  );
});
