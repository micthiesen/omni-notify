import { describe, expect, it } from "vitest";
import { VoiceEvidenceTracker } from "./voiceEvidence.js";

describe("VoiceEvidenceTracker", () => {
  it("requires repeated multi-window matches before confirmation", () => {
    const tracker = new VoiceEvidenceTracker();
    expect(tracker.observe("guest", 2, 1_000)).toBe("possible");
    expect(tracker.observe("guest", 2, 60_000)).toBe("confirmed");
  });

  it("does not combine matches more than five minutes apart", () => {
    const tracker = new VoiceEvidenceTracker();
    expect(tracker.observe("guest", 3, 1_000)).toBe("possible");
    expect(tracker.observe("guest", 3, 302_000)).toBe("possible");
  });

  it("forgets stale positive evidence after three misses", () => {
    const tracker = new VoiceEvidenceTracker();
    expect(tracker.observe("guest", 2, 1_000)).toBe("possible");
    expect(tracker.observe("guest", 0, 2_000)).toBe("none");
    expect(tracker.observe("guest", 1, 3_000)).toBe("none");
    expect(tracker.observe("guest", 0, 4_000)).toBe("none");
    expect(tracker.observe("guest", 2, 5_000)).toBe("possible");
  });
});
