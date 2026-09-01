import { describe, expect, it } from "@effect/vitest";
import { Effect, Exit } from "effect";
import { fromPromise, fromSync } from "./interop.js";

describe("Effect interop", () => {
  it.effect("maps rejected promises to an IntegrationError", () =>
    Effect.gen(function* () {
      const exit = yield* Effect.exit(
        fromPromise("fetch widget", () => Promise.reject(new Error("offline"))),
      );

      expect(Exit.isFailure(exit)).toBe(true);
      if (Exit.isFailure(exit)) {
        expect(String(exit.cause)).toContain("IntegrationError");
        expect(String(exit.cause)).toContain("fetch widget failed: offline");
      }
    }),
  );

  it.effect("maps thrown persistence failures to a PersistenceError", () =>
    Effect.gen(function* () {
      const exit = yield* Effect.exit(
        fromSync("save widget", () => {
          throw new Error("disk full");
        }),
      );

      expect(Exit.isFailure(exit)).toBe(true);
      if (Exit.isFailure(exit)) {
        expect(String(exit.cause)).toContain("PersistenceError");
        expect(String(exit.cause)).toContain("save widget failed: disk full");
      }
    }),
  );
});
