import { describe, expect, it } from "vitest";
import { extractBetweenTags } from "./parsing.js";

describe("extractBetweenTags", () => {
  it("extracts a well-formed tag pair", () => {
    expect(
      extractBetweenTags("<cleaned_article>hello</cleaned_article>", "cleaned_article"),
    ).toBe("hello");
  });

  it("trims surrounding whitespace", () => {
    expect(extractBetweenTags("<x>\n  body  \n</x>", "x")).toBe("body");
  });

  it("ignores preamble/trailing text outside the tags", () => {
    expect(extractBetweenTags("here you go:\n<x>body</x>\nthanks", "x")).toBe("body");
  });

  it("matches tags case-insensitively", () => {
    expect(extractBetweenTags("<X>body</X>", "x")).toBe("body");
  });

  it("recovers when the closing tag is missing (truncation)", () => {
    expect(extractBetweenTags("<x>the whole body was cut off", "x")).toBe(
      "the whole body was cut off",
    );
  });

  it("recovers when the closing tag lost its bracket", () => {
    expect(extractBetweenTags("<x>body text</x", "x")).toBe("body text");
  });

  it("throws when the opening tag is absent entirely", () => {
    expect(() => extractBetweenTags("no tags at all", "x")).toThrow(
      "Failed to extract content between <x> tags",
    );
  });

  it("throws when the body is empty", () => {
    expect(() => extractBetweenTags("<x></x>", "x")).toThrow();
  });
});
