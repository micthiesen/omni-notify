import { describe, expect, it } from "vitest";
import { simpleParser } from "mailparser";
import { extractEmailLinkMetadata } from "./linkMetadata.js";

async function metadata(body: string, headers: string[] = [], html = true) {
  const parsed = await simpleParser(
    [
      "From: Marketing <marketing@example.test>",
      "Message-ID: <fixture@example.test>",
      ...headers,
      `Content-Type: text/${html ? "html" : "plain"}; charset=utf-8`,
      "",
      body,
    ].join("\r\n"),
  );
  return extractEmailLinkMetadata(parsed);
}

describe("email link metadata", () => {
  it("caps unrelated header scanning and reports incomplete header evidence", async () => {
    const result = await metadata("hello", [
      ...Array.from({ length: 1001 }, () => "X-Unrelated: ignored"),
      "List-Unsubscribe: <https://example.test/unsubscribe>",
      "List-Unsubscribe-Post: List-Unsubscribe=One-Click",
    ]);
    expect(result.listUnsubscribe).toEqual({
      urls: [],
      post: null,
      present: false,
      truncated: true,
    });
  });
  it("decodes inert HTML anchors and deduplicates without fetching resources", async () => {
    const result = await metadata(
      '<img src="https://remote.test/pixel"><script>throw 1</script><a href="https://example.test/unsub?a=1&amp;b=2"> Unsubscribe &amp; preferences </a><a href="https://example.test/unsub?a=1&amp;b=2">again</a>',
    );
    expect(result.links).toEqual([
      {
        url: "https://example.test/unsub?a=1&b=2",
        label: "Unsubscribe & preferences",
        source: "html",
      },
    ]);
    expect(result.linksTruncated).toBe(false);
  });

  it("rejects unsafe schemes, credentials, relative and malformed URLs", async () => {
    const rejected = [
      "javascript:alert(1)",
      "data:text/html,hello",
      "/unsubscribe",
      "//example.test/path",
      "https://user:secret@example.test/",
      "https://",
      "https://exa mple.test/",
      "mailto:",
      "mailto:leave@example.test?subject=x%0d%0aBcc:other@example.test",
      "https://example.test/%00token",
      "https://example.test\\evil",
    ];
    const result = await metadata(
      rejected.map((url) => `<a href="${url}">Unsubscribe</a>`).join(""),
    );
    expect(result.links).toEqual([]);
  });

  it("extracts text URLs with no fabricated label", async () => {
    const result = await metadata(
      "Use https://example.test/unsubscribe?token=opaque or mailto:leave@example.test",
      [],
      false,
    );
    expect(result.links).toEqual([
      {
        url: "https://example.test/unsubscribe?token=opaque",
        label: "",
        source: "text",
      },
      { url: "mailto:leave@example.test", label: "", source: "text" },
    ]);
  });

  it("prioritizes subscription links and reports bounded omissions", async () => {
    const regular = Array.from(
      { length: 55 },
      (_, i) => `<a href="https://example.test/${i}">Shop</a>`,
    ).join("");
    const result = await metadata(
      `${regular}<a href="https://example.test/unsubscribe">Unsubscribe</a><a href="https://example.test/long">${"x".repeat(201)}</a><a href="https://example.test/${"x".repeat(4096)}">too long</a>`,
    );
    expect(result.links).toHaveLength(50);
    expect(result.links[0].label).toBe("Unsubscribe");
    expect(result.linksTruncated).toBe(true);
    expect(result.links.every((link) => link.url.length <= 4096)).toBe(true);
  });

  it("upgrades duplicate link labels before applying the cap", async () => {
    const regular = Array.from(
      { length: 55 },
      (_, i) => `<a href="https://example.test/${i}">Shop</a>`,
    ).join("");
    const result = await metadata(
      `${regular}<a href="https://example.test/54">Manage preferences</a>`,
    );
    expect(result.links[0]).toEqual({
      url: "https://example.test/54",
      label: "Manage preferences",
      source: "html",
    });
  });

  it("does not expose partial URLs at scan boundaries", async () => {
    const result = await metadata(
      `${" ".repeat(1024 * 1024 - 20)}<a href="https://example.test/unsubscribe?secret=abc">Unsubscribe</a>`,
    );
    expect(result.links).toEqual([]);
    expect(result.linksTruncated).toBe(true);
  });

  it("reads only unsubscribe headers, with folding and repeats", async () => {
    const result = await metadata("hello", [
      "X-Private-Token: secret",
      "List-Unsubscribe: <https://example.test/unsubscribe?token=opaque>,",
      " <mailto:leave@example.test?subject=unsubscribe>",
      "List-Unsubscribe: <https://example.test/unsubscribe?token=opaque>",
      "List-Unsubscribe-Post: List-Unsubscribe=One-Click",
    ]);
    expect(result.listUnsubscribe).toEqual({
      urls: [
        "https://example.test/unsubscribe?token=opaque",
        "mailto:leave@example.test?subject=unsubscribe",
      ],
      post: "List-Unsubscribe=One-Click",
      present: true,
      truncated: false,
    });
    expect(JSON.stringify(result)).not.toContain("secret");
  });

  it("requires an unambiguous exact one-click value", async () => {
    for (const headers of [
      ["List-Unsubscribe-Post: list-unsubscribe=one-click"],
      ["List-Unsubscribe-Post: List-Unsubscribe=One-Click; extra=value"],
      [
        "List-Unsubscribe-Post: List-Unsubscribe=One-Click",
        "List-Unsubscribe-Post: List-Unsubscribe=One-Click",
      ],
    ]) {
      expect((await metadata("hello", headers)).listUnsubscribe.post).toBeNull();
    }
  });

  it("bounds header URLs and header scans without shortening URL tokens", async () => {
    const result = await metadata("hello", [
      `List-Unsubscribe: ${Array.from({ length: 11 }, (_, i) => `<https://example.test/${i}>`).join(", ")}`,
      `List-Unsubscribe: <https://example.test/${"x".repeat(4096)}>`,
      `List-Unsubscribe: ${"x".repeat(16384)}`,
    ]);
    expect(result.listUnsubscribe.urls).toHaveLength(10);
    expect(result.listUnsubscribe.truncated).toBe(true);
  });

  it("reports absence without raw header output", async () => {
    expect((await metadata("hello")).listUnsubscribe).toEqual({
      urls: [],
      post: null,
      present: false,
      truncated: false,
    });
  });

  it("distinguishes post-only headers and suppresses one-click on incomplete headers", async () => {
    const postOnly = await metadata("hello", [
      "List-Unsubscribe-Post: List-Unsubscribe=One-Click",
    ]);
    expect(postOnly.listUnsubscribe.present).toBe(false);
    const incomplete = await metadata("hello", [
      "List-Unsubscribe-Post: List-Unsubscribe=One-Click",
      `List-Unsubscribe-Post: ${"x".repeat(16384)}`,
    ]);
    expect(incomplete.listUnsubscribe.post).toBeNull();
    expect(incomplete.listUnsubscribe.truncated).toBe(true);
  });
});
