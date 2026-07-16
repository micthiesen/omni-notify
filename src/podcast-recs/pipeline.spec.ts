import { describe, expect, it } from "vitest";
import type { PodcastWriteResult } from "./account.js";
import { toQueueResult } from "./pipeline.js";

describe("toQueueResult", () => {
  const cases: [PodcastWriteResult, string][] = [
    ["added", "queued"],
    ["already_exists", "already_queued"],
    ["not_found", "not_queued"],
    ["unavailable", "not_queued"],
    ["error", "not_queued"],
    ["removed", "not_queued"],
  ];

  for (const [input, expected] of cases) {
    it(`maps ${input} → ${expected}`, () => {
      expect(toQueueResult(input)).toBe(expected);
    });
  }
});
