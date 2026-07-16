import { describe, expect, it } from "vitest";
import { encodeCastroQueryValue } from "./api.js";

describe("Castro API query encoding", () => {
  it("uses the RFC 3986 encoding that is signed and sent on the wire", () => {
    expect(encodeCastroQueryValue("Beth's (History)!")).toBe(
      "Beth%27s%20%28History%29%21",
    );
    expect(encodeCastroQueryValue("https://example.com/feed?a=1&b=2")).toBe(
      "https%3A%2F%2Fexample.com%2Ffeed%3Fa%3D1%26b%3D2",
    );
  });
});
