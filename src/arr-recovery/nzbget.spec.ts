import { describe, expect, it } from "@effect/vitest";
import { Effect, Result } from "effect";
import { downloadHealth } from "./nzbget.js";
const item = {
  NZBID: 29,
  Status: "WARNING/HEALTH",
  Category: "sonarr",
  DestDir: "/tmp/inter/item",
  FinalDir: "",
  Parameters: [{ Name: "drone", Value: "download" }],
};
function fake(responses: unknown[]): typeof fetch {
  return (() =>
    Promise.resolve(
      new Response(JSON.stringify({ result: responses.shift() }), {
        headers: { "Content-Type": "application/json" },
      }),
    )) as typeof fetch;
}
describe("NZBGet terminal failure corroboration", () => {
  it.effect(
    "recognizes a parked health failure only for the exact Arr download and path",
    () =>
      Effect.gen(function* () {
        const result = yield* downloadHealth(
          "http://nzbget",
          "sonarr",
          "download",
          "/tmp/inter/item",
          fake([[], [item], []]),
        );
        expect(result).toBe("WARNING/HEALTH");
      }),
  );
  it.effect("does not touch an active or restarted download", () =>
    Effect.gen(function* () {
      expect(
        yield* downloadHealth(
          "http://nzbget",
          "sonarr",
          "download",
          "/tmp/inter/item",
          fake([[item]]),
        ),
      ).toBeUndefined();
      expect(
        yield* downloadHealth(
          "http://nzbget",
          "sonarr",
          "download",
          "/tmp/inter/item",
          fake([[], [item], [item]]),
        ),
      ).toBeUndefined();
    }),
  );
  it.effect("rejects another category, output path, or successful download", () =>
    Effect.gen(function* () {
      for (const changed of [
        { ...item, Category: "radarr" },
        { ...item, DestDir: "/tmp/inter/other" },
        { ...item, Status: "SUCCESS/ALL" },
      ]) {
        expect(
          yield* downloadHealth(
            "http://nzbget",
            "sonarr",
            "download",
            "/tmp/inter/item",
            fake([[], [changed]]),
          ),
        ).toBeUndefined();
      }
    }),
  );
  it.effect("fails closed for malformed RPC responses", () =>
    Effect.gen(function* () {
      const result = yield* Effect.result(
        downloadHealth(
          "http://nzbget",
          "sonarr",
          "download",
          "/tmp/inter/item",
          fake([{}]),
        ),
      );
      expect(Result.isFailure(result)).toBe(true);
    }),
  );
});
