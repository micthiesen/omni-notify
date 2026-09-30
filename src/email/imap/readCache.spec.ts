import { describe, expect, it } from "vitest";
import { BoundedReadCache } from "./readCache.js";

describe("BoundedReadCache", () => {
  it("expires entries and refreshes recency when enforcing the count bound", () => {
    const cache = new BoundedReadCache<string>(2, 20, 30);
    cache.set("a", "A", 1, 0);
    cache.set("b", "B", 1, 0);
    expect(cache.get("a", 1)).toBe("A");
    cache.set("c", "C", 1, 2);
    expect(cache.get("b", 2)).toBeUndefined();
    expect(cache.get("a", 2)).toBe("A");
    expect(cache.get("a", 31)).toBeUndefined();
  });

  it("bounds bytes and clears retained values", () => {
    const cache = new BoundedReadCache<string>(5, 3);
    cache.set("too-large", "x", 4, 0);
    expect(cache.get("too-large", 0)).toBeUndefined();
    cache.set("a", "a", 2, 0);
    cache.set("b", "b", 2, 0);
    expect(cache.get("a", 0)).toBeUndefined();
    expect(cache.get("b", 0)).toBe("b");
    cache.clear();
    expect(cache.get("b", 0)).toBeUndefined();
  });
});
