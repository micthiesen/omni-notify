import { describe, expect, it } from "vitest";
import { decodeCrdtDocument, encodeCrdtDocument } from "./codec.js";

describe("reminder CRDT text integrity", () => {
  it("preserves separators and controls through encoding and readback", () => {
    const value = "Title\u2028second line\u2029paragraph\u0001\t🙂";
    expect(decodeCrdtDocument(encodeCrdtDocument(value))).toBe(value);
  });
});
