import { describe, expect, it } from "vitest";
import { parseByteRange } from "./routes.js";

describe("parseByteRange", () => {
  it("returns undefined without a Range header", () => {
    expect(parseByteRange(undefined, 100)).toBeUndefined();
  });

  it("parses a bounded range", () => {
    expect(parseByteRange("bytes=0-49", 100)).toEqual({ start: 0, end: 49 });
  });

  it("clamps the end to the file size", () => {
    expect(parseByteRange("bytes=50-1000", 100)).toEqual({ start: 50, end: 99 });
  });

  it("parses an open-ended range", () => {
    expect(parseByteRange("bytes=10-", 100)).toEqual({ start: 10, end: 99 });
  });

  it("parses a suffix range", () => {
    expect(parseByteRange("bytes=-25", 100)).toEqual({ start: 75, end: 99 });
  });

  it("rejects out-of-bounds and malformed ranges", () => {
    expect(parseByteRange("bytes=100-", 100)).toBe("invalid");
    expect(parseByteRange("bytes=-0", 100)).toBe("invalid");
    expect(parseByteRange("bytes=-", 100)).toBe("invalid");
    expect(parseByteRange("chunks=0-5", 100)).toBe("invalid");
  });
});
