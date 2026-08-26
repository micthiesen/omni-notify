import { describe, expect, it } from "vitest";
import { decideVoiceMatchAction } from "./presencePolicy.js";

const confirmed = {
  state: "confirmed" as const,
  confidence: 0.71,
  detectedAt: 1_000,
  reason: "Live conversation confirmed",
};

describe("decideVoiceMatchAction", () => {
  it("never downgrades a current confirmation to possible", () => {
    expect(decideVoiceMatchAction("possible", confirmed)).toBe("retain_confirmed");
  });

  it("reuses a current confirmation instead of paying to verify it again", () => {
    expect(decideVoiceMatchAction("confirmed", confirmed)).toBe("retain_confirmed");
  });

  it("still requires verification for newly confirmed repeated evidence", () => {
    expect(decideVoiceMatchAction("confirmed", undefined)).toBe("verify");
  });
});
