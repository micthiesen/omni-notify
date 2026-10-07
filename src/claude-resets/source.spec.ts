import { Schema } from "effect";
import { describe, expect, it } from "vitest";
import { ClaudeResetSourceSchema } from "./source.js";

const event = {
  id: "2026-09-22-saved-reset",
  date: "2026-09-22T16:31:00Z",
  type: "counter-reset",
  status: "historic",
  confidence: "confirmed",
  plans: ["all"],
  surfaces: ["claude-code"],
  title: "A saved reset",
  summary: "A saved reset was announced.",
  sources: [{ url: "https://www.anthropic.com/claude-opus-5-5" }],
};
const decode = Schema.decodeUnknownSync(ClaudeResetSourceSchema);
const source = (events: unknown[] = [event]) => ({ updated: "2026-10-07", events });

describe("Claude reset source validation", () => {
  it("reads the live catalog shape and discards unrelated metadata", () => {
    const result = decode({ ...source(), version: "1.0.0" });
    expect(result.events).toEqual([event]);
    expect(result).not.toHaveProperty("version");
  });

  it("retains unknown classifications for fail-closed selection", () => {
    expect(decode(source([{ ...event, type: "future-kind" }])).events).toHaveLength(1);
  });

  it("rejects missing catalog data", () => {
    expect(() => decode({ updated: "2026-10-07" })).toThrow();
  });

  it("rejects invalid times, unsafe URLs and oversized fields", () => {
    for (const patch of [
      { date: "yesterday" },
      { sources: [{ url: "http://example.com" }] },
      { sources: [{ url: "https://user:password@example.com" }] },
      { id: "x".repeat(257) },
      { summary: "x".repeat(16_385) },
      { plans: Array.from({ length: 51 }, () => "all") },
      { sources: Array.from({ length: 51 }, () => event.sources[0]) },
    ])
      expect(() => decode(source([{ ...event, ...patch }]))).toThrow();
  });

  it("caps the catalog size", () => {
    expect(() => decode(source(Array.from({ length: 5_001 }, () => event)))).toThrow();
  });
});
