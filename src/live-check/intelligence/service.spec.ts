import { describe, expect, it } from "vitest";
import { Platform } from "../platforms/index.js";
import type { Streamer } from "../streamers.js";
import { isDestinyOwnedStream } from "./service.js";

function streamer(overrides: Partial<Streamer>): Streamer {
  return {
    id: "dgg:youtube:video-id",
    displayName: "Guest Channel",
    bindings: [{ platform: Platform.YouTube, username: "video-id" }],
    tier: "background",
    dgg: { hosted: false, viewers: 100 },
    ...overrides,
  };
}

describe("isDestinyOwnedStream", () => {
  it("rejects the configured Destiny stream", () => {
    expect(isDestinyOwnedStream(streamer({ id: "destiny" }))).toBe(true);
  });

  it("rejects a DGG-discovered Destiny video with a dynamic ID", () => {
    expect(
      isDestinyOwnedStream(
        streamer({ id: "dgg:youtube:abc123", displayName: "Destiny" }),
      ),
    ).toBe(true);
  });

  it("rejects canonical Destiny usernames regardless of display name", () => {
    expect(
      isDestinyOwnedStream(
        streamer({
          bindings: [{ platform: Platform.YouTube, username: "@Destiny" }],
        }),
      ),
    ).toBe(true);
  });

  it("keeps a third-party DGG stream eligible", () => {
    expect(isDestinyOwnedStream(streamer({}))).toBe(false);
  });
});
