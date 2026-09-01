import { describe, expect, it } from "@effect/vitest";
import { Effect, Exit } from "effect";
import { integrationEffect, RecommendationIntegrationError } from "./effect.js";

describe("recommendation Effect adapters", () => {
  it.effect("accepts synchronous third-party test doubles", () =>
    Effect.gen(function* () {
      const value = yield* integrationEffect("sync adapter", () => 42);
      expect(value).toBe(42);
    }),
  );

  it.effect("preserves the integration operation in the typed error", () =>
    Effect.gen(function* () {
      const exit = yield* Effect.exit(
        integrationEffect("TMDB lookup", () => Promise.reject(new Error("offline"))),
      );
      expect(Exit.isFailure(exit)).toBe(true);
      if (Exit.isFailure(exit)) {
        const failure = exit.cause;
        expect(String(failure)).toContain("TMDB lookup failed: offline");
      }
    }),
  );

  it("uses a tagged integration error", () => {
    const error = new RecommendationIntegrationError({
      operation: "Plex history",
      cause: new Error("timed out"),
    });
    expect(error._tag).toBe("RecommendationIntegrationError");
  });
});
