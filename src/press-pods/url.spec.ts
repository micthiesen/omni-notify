import { describe, expect, it } from "vitest";
import { normalizeUrl } from "./url.js";

describe("normalizeUrl", () => {
  it("strips utm_* and known tracking params", () => {
    expect(
      normalizeUrl("https://example.com/a?utm_source=x&utm_medium=ios&ref=abc"),
    ).toBe("https://example.com/a");
  });

  it("keeps content-bearing query params", () => {
    expect(normalizeUrl("https://example.com/?p=123&utm_campaign=x")).toBe(
      "https://example.com/?p=123",
    );
  });

  it("drops the fragment", () => {
    expect(normalizeUrl("https://example.com/a#section-2")).toBe(
      "https://example.com/a",
    );
  });

  it("lowercases scheme and host and strips a leading www.", () => {
    expect(normalizeUrl("HTTPS://WWW.Example.COM/Path")).toBe(
      "https://example.com/Path",
    );
  });

  it("removes a trailing slash but keeps the root slash", () => {
    expect(normalizeUrl("https://example.com/a/b/")).toBe("https://example.com/a/b");
    expect(normalizeUrl("https://example.com/")).toBe("https://example.com/");
  });

  it("orders remaining params so param order doesn't change identity", () => {
    expect(normalizeUrl("https://example.com/a?b=2&a=1")).toBe(
      normalizeUrl("https://example.com/a?a=1&b=2"),
    );
  });

  it("collapses two tracking-only variants of the same article to one identity", () => {
    const a = normalizeUrl("https://www.natesilver.net/p/x?r=7esws&utm_medium=ios");
    const b = normalizeUrl(
      "https://natesilver.net/p/x?utm_medium=ios&r=7esws&triedRedirect=true",
    );
    expect(a).toBe(b);
  });

  it("collapses http and https variants of the same article", () => {
    expect(normalizeUrl("http://example.com/a")).toBe(
      normalizeUrl("https://example.com/a"),
    );
  });

  it("strips a trailing DNS-root dot from the host", () => {
    expect(normalizeUrl("https://example.com./a")).toBe(
      normalizeUrl("https://example.com/a"),
    );
  });

  it("keeps genuinely different articles distinct", () => {
    expect(normalizeUrl("https://example.com/a")).not.toBe(
      normalizeUrl("https://example.com/b"),
    );
  });

  it("canonicalizes X share links by status ID", () => {
    expect(
      normalizeUrl(
        "https://x.com/edels0n/status/2077031491045929255?s=46&t=LN32clxPq8AlS6Ujqu_UEg",
      ),
    ).toBe("https://x.com/i/status/2077031491045929255");
  });

  it("collapses X and Twitter host, username, and media-path variants", () => {
    const canonical = "https://x.com/i/status/2077031491045929255";
    expect(
      normalizeUrl(
        "https://mobile.twitter.com/old_handle/status/2077031491045929255/photo/1",
      ),
    ).toBe(canonical);
    expect(
      normalizeUrl("https://www.x.com/new_handle/status/2077031491045929255"),
    ).toBe(canonical);
    expect(
      normalizeUrl("https://x.com/i/web/status/2077031491045929255?utm_source=share"),
    ).toBe(canonical);
  });

  it("keeps short query parameters on non-X URLs", () => {
    expect(normalizeUrl("https://example.com/article?s=46&t=chapter-2")).toBe(
      "https://example.com/article?s=46&t=chapter-2",
    );
  });

  it("returns the trimmed input when the string is not a URL", () => {
    expect(normalizeUrl("  not a url  ")).toBe("not a url");
  });
});
