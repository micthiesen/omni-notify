import { describe, expect, it } from "vitest";
import { removeExtraEmptyLines } from "./lineFiltering.js";

describe("removeExtraEmptyLines", () => {
  it("keeps non-empty lines", () => {
    expect(removeExtraEmptyLines(["a", "b"])).toEqual(["a", "b"]);
  });

  it("drops empty quote lines", () => {
    expect(removeExtraEmptyLines(["> a", "> ", "> b"])).toEqual(["> a", "> b"]);
  });

  it("drops leading and trailing empty lines", () => {
    expect(removeExtraEmptyLines(["", "a", ""])).toEqual(["a"]);
  });

  it("collapses runs of empty lines between prose", () => {
    expect(removeExtraEmptyLines(["a", "", "", "b"])).toEqual(["a", "", "b"]);
  });

  it("keeps a single separator between paragraphs", () => {
    expect(removeExtraEmptyLines(["a", "", "b"])).toEqual(["a", "", "b"]);
  });
});
