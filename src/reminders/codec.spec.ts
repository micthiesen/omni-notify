import { describe, expect, it } from "vitest";
import { gzipSync, inflateSync } from "node:zlib";
import { decodeCrdtDocument, encodeCrdtDocument } from "./codec.js";

describe("reminder CRDT text integrity", () => {
  it.each(["gzip", "raw"])("decodes server-decrypted %s documents", (format) => {
    const text = "ADP reminder 🌿";
    const proto = inflateSync(Buffer.from(encodeCrdtDocument(text), "base64"));
    const wire = format === "gzip" ? gzipSync(proto) : proto;
    expect(decodeCrdtDocument(wire.toString("base64"))).toBe(text);
  });

  it("rejects ciphertext and oversized decompression", () => {
    expect(() => decodeCrdtDocument(Buffer.alloc(64, 3).toString("base64"))).toThrow();
    expect(() =>
      decodeCrdtDocument(gzipSync(Buffer.alloc(256 * 1024, 65)).toString("base64")),
    ).toThrow();
  });
  it("preserves separators and controls through encoding and readback", () => {
    const value = "Title\u2028second line\u2029paragraph\u0001\t🙂";
    expect(decodeCrdtDocument(encodeCrdtDocument(value))).toBe(value);
  });
});
