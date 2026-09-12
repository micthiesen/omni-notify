import { describe, expect, it } from "@effect/vitest";
import { Effect, Result } from "effect";
import { vi } from "vitest";
const read = vi.hoisted(() => vi.fn());
vi.mock("node:fs/promises", () => ({ readdir: (...args: unknown[]) => read(...args) }));
import { verifyDownloadRemoved } from "./filesystem.js";
describe("read-only deletion verification", () => {
  it.effect("proves deletion even when its parent is empty", () =>
    Effect.gen(function* () {
      read.mockResolvedValueOnce([]);
      expect(yield* verifyDownloadRemoved("/tmp/inter/failed-job")).toBe(true);
    }),
  );
  it.effect("does not accept an inaccessible parent as deletion", () =>
    Effect.gen(function* () {
      read.mockRejectedValueOnce(new Error("EACCES"));
      expect(
        Result.isFailure(
          yield* Effect.result(verifyDownloadRemoved("/tmp/inter/failed-job")),
        ),
      ).toBe(true);
    }),
  );
  it.effect("keeps existing data unverified", () =>
    Effect.gen(function* () {
      read.mockResolvedValueOnce(["failed-job"]);
      expect(yield* verifyDownloadRemoved("/tmp/inter/failed-job")).toBe(false);
    }),
  );
  it.effect("rejects library and parent traversal paths", () =>
    Effect.gen(function* () {
      for (const path of [
        "/media/storage/sonarr/Show",
        "/tmp/inter/../../library",
        "/tmp/inter",
      ])
        expect(
          Result.isFailure(yield* Effect.result(verifyDownloadRemoved(path))),
        ).toBe(true);
    }),
  );
});
