import { describe, expect, it } from "vitest";
import {
  notificationBatches,
  notificationMessage,
  sourcePathsConfined,
} from "./service.js";
import type { Evidence } from "./types.js";
import type { RecoveryAction } from "./persistence.js";
describe("action boundaries", () => {
  it("never drops an action from oversized Pushover batches", () => {
    const actions = Array.from(
      { length: 40 },
      (_, i) =>
        ({
          title: `Release ${i}`,
          phase: "done",
          target: { title: `Series ${i} ${"x".repeat(500)}` },
          decision: { action: "import", source: "llm" },
        }) as RecoveryAction,
    );
    const batches = notificationBatches(actions);
    expect(batches.flat()).toEqual(actions);
    expect(batches.every((batch) => notificationMessage(batch).length <= 1000)).toBe(
      true,
    );
  });
  it("groups many imports for one series into a single notification", () => {
    const actions = Array.from(
      { length: 46 },
      () =>
        ({
          phase: "done",
          target: { title: "House of Cards" },
          decision: { action: "import", source: "rules" },
        }) as RecoveryAction,
    );
    expect(notificationBatches(actions)).toHaveLength(1);
    expect(notificationMessage(actions)).toContain("46 downloads");
  });
  it("rejects source paths in, containing, or equal to the library", () => {
    for (const outputPath of [
      "/media/storage/sonarr/Show",
      "/media/storage/sonarr",
      "/media/storage/sonarr/Show/subdir",
    ]) {
      const e = {
        items: [{ outputPath }],
        target: { path: "/media/storage/sonarr/Show" },
        files: [],
      } as unknown as Evidence;
      expect(sourcePathsConfined(e)).toBe(false);
    }
  });
});
