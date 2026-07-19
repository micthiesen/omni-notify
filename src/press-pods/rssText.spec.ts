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

  it("renders ## chapter markers as bold headings, not literal markdown", () => {
    expect(prepareTextForRss("## Background\nbody text")).toBe(
      "<b>Background</b><br>body text",
    );
  });

  it("does not treat mid-line ## as a heading", () => {
    expect(prepareTextForRss("rated it 4## stars")).toBe("rated it 4## stars");
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
