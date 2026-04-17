import { describe, expect, it } from "vitest";
import { Platform } from "./platforms/index.js";
import { buildStreamers, normalizeId } from "./streamers.js";

describe("normalizeId", () => {
  it("lowercases and trims display names", () => {
    expect(normalizeId("  Destiny  ")).toBe("destiny");
    expect(normalizeId("DESTINY")).toBe("destiny");
  });
});

describe("buildStreamers", () => {
  it("merges entries with the same display name across platforms", () => {
    const result = buildStreamers(
      [
        [Platform.YouTube, [{ username: "@destiny2", displayName: "Destiny" }]],
        [Platform.Kick, [{ username: "destiny", displayName: "Destiny" }]],
      ],
      {},
    );
    expect(result).toHaveLength(1);
    expect(result[0].id).toBe("destiny");
    expect(result[0].displayName).toBe("Destiny");
    expect(result[0].bindings).toEqual([
      { platform: Platform.YouTube, username: "@destiny2" },
      { platform: Platform.Kick, username: "destiny" },
    ]);
  });

  it("is case-insensitive when merging display names", () => {
    const result = buildStreamers(
      [
        [Platform.Twitch, [{ username: "a", displayName: "destiny" }]],
        [Platform.Kick, [{ username: "b", displayName: "DESTINY" }]],
      ],
      {},
    );
    expect(result).toHaveLength(1);
    expect(result[0].bindings).toHaveLength(2);
  });

  it("keeps distinct streamers when display names differ", () => {
    const result = buildStreamers(
      [
        [Platform.Twitch, [{ username: "shroud", displayName: "Shroud" }]],
        [Platform.Kick, [{ username: "destiny", displayName: "Destiny" }]],
      ],
      {},
    );
    expect(result.map((s) => s.displayName).sort()).toEqual(["Destiny", "Shroud"]);
  });

  it("throws on duplicate (platform, username) bindings", () => {
    expect(() =>
      buildStreamers(
        [
          [
            Platform.Twitch,
            [
              { username: "shroud", displayName: "Shroud" },
              { username: "shroud", displayName: "OtherName" },
            ],
          ],
        ],
        {},
      ),
    ).toThrow(/Duplicate platform binding/);
  });

  it("applies pushoverToken override by streamer id (case-insensitive)", () => {
    const result = buildStreamers(
      [[Platform.Kick, [{ username: "destiny", displayName: "Destiny" }]]],
      { DESTINY: { pushoverToken: "tok-abc" } },
    );
    expect(result[0].pushoverToken).toBe("tok-abc");
  });

  it("ignores overrides for unknown streamers", () => {
    const result = buildStreamers(
      [[Platform.Kick, [{ username: "destiny", displayName: "Destiny" }]]],
      { SomeoneElse: { pushoverToken: "tok" } },
    );
    expect(result).toHaveLength(1);
    expect(result[0].pushoverToken).toBeUndefined();
  });
});
