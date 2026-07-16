import { describe, expect, it } from "vitest";
import { podcastIndexAuthHash, podcastIndexAuthHeaders } from "./auth.js";

describe("Podcast Index authentication", () => {
  it("is deterministic for the same inputs", () => {
    const a = podcastIndexAuthHash("my-key", "my-secret", "1234567890");
    const b = podcastIndexAuthHash("my-key", "my-secret", "1234567890");
    expect(a).toBe(b);
  });

  it("returns a 40-character lowercase hex sha1 digest", () => {
    const hash = podcastIndexAuthHash("my-key", "my-secret", "1234567890");
    expect(hash).toMatch(/^[0-9a-f]{40}$/);
  });

  it("changes when any input changes", () => {
    const base = podcastIndexAuthHash("my-key", "my-secret", "1234567890");
    expect(podcastIndexAuthHash("other-key", "my-secret", "1234567890")).not.toBe(base);
    expect(podcastIndexAuthHash("my-key", "other-secret", "1234567890")).not.toBe(base);
    expect(podcastIndexAuthHash("my-key", "my-secret", "1234567891")).not.toBe(base);
  });

  it("returns all four required headers", () => {
    const nowMs = 1_700_000_000_000;
    const headers = podcastIndexAuthHeaders(
      { key: "my-key", secret: "my-secret" },
      nowMs,
    );

    expect(headers["X-Auth-Key"]).toBe("my-key");
    expect(headers["X-Auth-Date"]).toBe(String(Math.floor(nowMs / 1000)));
    expect(headers.Authorization).toBe(
      podcastIndexAuthHash("my-key", "my-secret", String(Math.floor(nowMs / 1000))),
    );
    expect(headers["User-Agent"]).toBe("omni-notify/1.0");
  });

  it("computes X-Auth-Date as floor(nowMs / 1000)", () => {
    const headers = podcastIndexAuthHeaders(
      { key: "k", secret: "s" },
      1_700_000_000_999,
    );
    expect(headers["X-Auth-Date"]).toBe("1700000000");
  });
});
