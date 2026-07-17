import { describe, expect, it } from "vitest";
import { MAX_CARRIER_CANDIDATES, selectValidCandidates } from "./candidates.js";

const VALID_CODES = new Set(["dicom", "gls", "ups", "canpost", "fedex"]);

describe("selectValidCandidates", () => {
  it("keeps valid candidates in ranked order", () => {
    const result = selectValidCandidates(["dicom", "gls", "canpost"], VALID_CODES);
    expect(result.valid).toEqual(["dicom", "gls", "canpost"]);
    expect(result.invalid).toEqual([]);
  });

  it("splits out invalid candidates without breaking rank order", () => {
    const result = selectValidCandidates(["bogus", "dicom", "gls"], VALID_CODES);
    expect(result.valid).toEqual(["dicom", "gls"]);
    expect(result.invalid).toEqual(["bogus"]);
  });

  it("returns empty valid list when nothing matches", () => {
    const result = selectValidCandidates(["nope", "nada"], VALID_CODES);
    expect(result.valid).toEqual([]);
    expect(result.invalid).toEqual(["nope", "nada"]);
  });

  it("dedupes repeated candidates, keeping the first occurrence", () => {
    const result = selectValidCandidates(["dicom", "gls", "dicom"], VALID_CODES);
    expect(result.valid).toEqual(["dicom", "gls"]);
  });

  it("trims whitespace and drops blank entries", () => {
    const result = selectValidCandidates([" dicom ", "", "  "], VALID_CODES);
    expect(result.valid).toEqual(["dicom"]);
    expect(result.invalid).toEqual([]);
  });

  it("caps valid candidates at MAX_CARRIER_CANDIDATES", () => {
    const result = selectValidCandidates(
      ["dicom", "gls", "ups", "canpost", "fedex"],
      VALID_CODES,
    );
    expect(result.valid).toHaveLength(MAX_CARRIER_CANDIDATES);
    expect(result.valid).toEqual(["dicom", "gls", "ups"]);
  });

  it("handles empty input", () => {
    const result = selectValidCandidates([], VALID_CODES);
    expect(result.valid).toEqual([]);
    expect(result.invalid).toEqual([]);
  });
});
