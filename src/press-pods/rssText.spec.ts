import { describe, expect, it } from "vitest";
import { isBlockQuote, prepareTextForRss } from "./rssText.js";

describe("prepareTextForRss", () => {
  it("escapes XML entities", () => {
    expect(prepareTextForRss("a & b <c>")).toBe("a &amp; b &lt;c&gt;");
  });

  it("italicizes blockquote lines and strips the quote prefix", () => {
    expect(prepareTextForRss("intro\n> quoted text")).toBe(
      "intro<br><i>quoted text</i>",
    );
  });

  it("returns empty string for undefined", () => {
    expect(prepareTextForRss(undefined)).toBe("");
  });
});

describe("isBlockQuote", () => {
  it("detects escaped quote prefixes", () => {
    expect(isBlockQuote("&gt; quoted")).toBe(true);
    expect(isBlockQuote("plain")).toBe(false);
  });
});
