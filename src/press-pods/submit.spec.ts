import { describe, expect, it } from "vitest";
import { submitEpisodeSchema } from "./submit.js";

describe("submitEpisodeSchema", () => {
  it("parses a valid URL", () => {
    const result = submitEpisodeSchema.parse({
      url: "https://www.theatlantic.com/culture/archive/2022/04/article/629608/",
    });
    expect(result.url).toBe(
      "https://www.theatlantic.com/culture/archive/2022/04/article/629608/",
    );
  });

  it("trims whitespace from URL", () => {
    const result = submitEpisodeSchema.parse({
      url: "  https://example.com/article  ",
    });
    expect(result.url).toBe("https://example.com/article");
  });

  it("extracts first URL when duplicated with newline", () => {
    const result = submitEpisodeSchema.parse({
      url: "https://example.com/article\nhttps://example.com/article",
    });
    expect(result.url).toBe("https://example.com/article");
  });

  it("extracts first URL when followed by trailing newline", () => {
    const result = submitEpisodeSchema.parse({
      url: "https://example.com/article\n",
    });
    expect(result.url).toBe("https://example.com/article");
  });

  it("rejects non-URL strings", () => {
    expect(() => submitEpisodeSchema.parse({ url: "not a url" })).toThrow();
  });

  it("rejects empty string", () => {
    expect(() => submitEpisodeSchema.parse({ url: "" })).toThrow();
  });

  it("rejects missing url field", () => {
    expect(() => submitEpisodeSchema.parse({})).toThrow();
  });

  it.each([
    "http://localhost/article",
    "http://127.0.0.1/article",
    "http://[::1]/article",
    "file:///etc/passwd",
  ])("rejects non-public URL %s", (url) => {
    expect(() => submitEpisodeSchema.parse({ url })).toThrow();
  });
});
