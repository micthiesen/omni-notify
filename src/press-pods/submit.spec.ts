import type { NamedLogger as Logger } from "@micthiesen/mitools/logging";
import { Effect } from "effect";
import { describe, expect, it, vi } from "vitest";
import { runTest } from "../live-check/testRuntime.js";
import { PressPodsError } from "./effect.js";

const mocks = vi.hoisted(() => ({
  enqueue: vi.fn(),
}));

vi.mock("@micthiesen/mitools/karakeep", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@micthiesen/mitools/karakeep")>()),
  addBookmark: vi.fn(() => Effect.succeed("bookmark")),
}));
vi.mock("./persistence.js", () => ({
  PressPodsPersistence: {
    findActiveJobByNormalizedUrl: vi.fn(() => Effect.succeed(undefined)),
    findFailedJobByNormalizedUrl: vi.fn(() => Effect.succeed(undefined)),
    requeueJobNow: vi.fn(() => Effect.succeed(undefined)),
    enqueueEpisodeJob: mocks.enqueue,
  },
}));
vi.mock("./publicHttp.js", () => ({
  assertPublicHttpUrlSyntax: (value: string) => new URL(value),
  assertPublicHttpUrl: () =>
    Effect.fail(
      new PressPodsError({
        operation: "validate public PressPods URL",
        cause: new Error("host resolves to a private address"),
      }),
    ),
}));

import { submitEpisodeUrlEffect } from "./submit.js";

describe("PressPods submission validation", () => {
  it("rejects DNS-private hosts before durable enqueue or bookmarking", async () => {
    const logger = { info: vi.fn(() => Effect.void) } as unknown as Logger;

    await expect(
      runTest(
        submitEpisodeUrlEffect("https://internal.example/article", vi.fn(), logger),
      ),
    ).rejects.toThrow("host resolves to a private address");
    expect(mocks.enqueue).not.toHaveBeenCalled();
  });
});
