import { describe, expect, it } from "vitest";
import { VoiceEvidenceTracker } from "./voiceEvidence.js";

describe("VoiceEvidenceTracker", () => {
  it("requires matches in two independent samples before confirmation", () => {
    const tracker = new VoiceEvidenceTracker();
    expect(tracker.observe("guest", 1, 4, 1_000)).toBe("possible");
    expect(tracker.observe("guest", 1, 4, 60_000)).toBe("confirmed");
  });

  it("does not combine matches more than ten minutes apart", () => {
    const tracker = new VoiceEvidenceTracker();
    expect(tracker.observe("guest", 3, 4, 1_000)).toBe("possible");
    expect(tracker.observe("guest", 3, 4, 602_000)).toBe("possible");
  });

  it("does not treat no-speech VAD windows as negative evidence", () => {
    const tracker = new VoiceEvidenceTracker();
    expect(tracker.observe("guest", 1, 4, 1_000)).toBe("possible");
    for (let index = 1; index <= 8; index += 1) {
      expect(tracker.observe("guest", 0, 0, index * 60_000)).toBe("none");
    }
    expect(tracker.observe("guest", 1, 4, 9 * 60_000)).toBe("confirmed");
  });

  it("forgets positive evidence after five speech-negative scans", () => {
    const tracker = new VoiceEvidenceTracker();
    expect(tracker.observe("guest", 1, 4, 1_000)).toBe("possible");
    for (let index = 1; index <= 5; index += 1) {
      expect(tracker.observe("guest", 0, 4, index * 10_000)).toBe("none");
    }
    expect(tracker.observe("guest", 1, 4, 60_000)).toBe("possible");
  });

  it("does not count a speech sample without a matching window", () => {
    const tracker = new VoiceEvidenceTracker();
    expect(tracker.observe("guest", 0, 4, 1_000)).toBe("none");
    expect(tracker.observe("guest", 1, 4, 60_000)).toBe("possible");
  });
});
