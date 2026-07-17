import { describe, expect, it } from "vitest";
import { fingerprintEvidence } from "./fingerprint.js";

const a = {
  evidenceId: "a",
  kind: "listen",
  showTitle: "Show A",
  observedAt: 100,
  starred: true,
};
const b = {
  evidenceId: "b",
  kind: "listen",
  showTitle: "Show B",
  observedAt: 200,
  completion: undefined,
};

describe("fingerprintEvidence", () => {
  it("produces a pinned, byte-stable fingerprint (persisted in checkpoints)", () => {
    // Fingerprints are compared against stored values; this hash must never
    // change for the same logical evidence.
    expect(fingerprintEvidence([b, a])).toBe("f09fcee7d85f7dc9f02e0af0");
  });

  it("is independent of input order and undefined fields", () => {
    expect(fingerprintEvidence([a, b])).toBe(fingerprintEvidence([b, a]));
    expect(fingerprintEvidence([a, { ...b, completion: undefined }])).toBe(
      fingerprintEvidence([a, b]),
    );
  });

  it("changes when a defined field changes", () => {
    expect(fingerprintEvidence([a, b])).not.toBe(
      fingerprintEvidence([a, { ...b, completion: 0.5 }]),
    );
  });
});
