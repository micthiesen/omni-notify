import { describe, expect, it } from "vitest";
import {
  assertPublicHttpUrlSyntax,
  createPublicDnsLookup,
  isPublicAddress,
} from "./publicHttp.js";

describe("PressPods public HTTP guard", () => {
  it.each([
    "127.0.0.1",
    "10.0.0.1",
    "100.64.0.1",
    "169.254.169.254",
    "172.16.0.1",
    "192.168.1.1",
    "224.0.0.1",
    "::1",
    "fc00::1",
    "fe80::1",
    "::ffff:127.0.0.1",
    "::ffff:7f00:1",
    "64:ff9b::7f00:1",
  ])("rejects non-public address %s", (address) => {
    expect(isPublicAddress(address)).toBe(false);
  });

  it.each(["1.1.1.1", "8.8.8.8", "2606:4700:4700::1111"])(
    "allows public address %s",
    (address) => {
      expect(isPublicAddress(address)).toBe(true);
    },
  );

  it.each([
    "file:///etc/passwd",
    "ftp://example.com/article",
    "http://localhost/article",
    "http://service.localhost/article",
    "http://127.0.0.1/article",
    "http://[::1]/article",
    "https://user:password@example.com/article",
  ])("rejects unsafe URL %s before any request", (url) => {
    expect(() => assertPublicHttpUrlSyntax(url)).toThrow();
  });

  it("allows a public HTTP URL", () => {
    expect(assertPublicHttpUrlSyntax("https://example.com/article").href).toBe(
      "https://example.com/article",
    );
  });

  it("rejects a private address returned at connection time", async () => {
    const resolver = ((_hostname, _options, callback) => {
      callback(null, "127.0.0.1", 4);
    }) as Parameters<typeof createPublicDnsLookup>[0];
    const guardedLookup = createPublicDnsLookup(resolver);

    await expect(
      new Promise<void>((resolve, reject) => {
        guardedLookup("redirect.example", { family: 4 }, (error) => {
          if (error) reject(error);
          else resolve();
        });
      }),
    ).rejects.toThrow("resolve only to public addresses");
  });
});
