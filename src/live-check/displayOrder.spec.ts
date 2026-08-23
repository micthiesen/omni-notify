import { describe, expect, it } from "vitest";
import { sortLiveDisplay, sortOfflineDisplay } from "./displayOrder.js";

describe("display ordering", () => {
  it("keeps primary channels ahead of hotter background channels", () => {
    const ordered = sortLiveDisplay([
      {
        id: "background",
        tier: "background",
        viewerCount: 50_000,
        maxViewerCount: 50_000,
      },
      { id: "primary-cool", tier: "primary", viewerCount: 10, maxViewerCount: 20 },
      { id: "primary-hot", tier: "primary", viewerCount: 20, maxViewerCount: 30 },
    ]);
    expect(ordered.map((item) => item.id)).toEqual([
      "primary-hot",
      "primary-cool",
      "background",
    ]);
  });

  it("uses session peak when current viewers are unavailable and preserves ties", () => {
    const ordered = sortLiveDisplay([
      { id: "first-tie", tier: "primary", viewerCount: null, maxViewerCount: 100 },
      { id: "second-tie", tier: "primary", viewerCount: 100, maxViewerCount: 120 },
      { id: "cooler", tier: "primary", viewerCount: null, maxViewerCount: 20 },
    ]);
    expect(ordered.map((item) => item.id)).toEqual([
      "first-tie",
      "second-tie",
      "cooler",
    ]);
  });

  it("uses a rank-only viewer override without replacing actual viewers", () => {
    const ordered = sortLiveDisplay([
      {
        id: "dgg-large-platform",
        tier: "background",
        viewerCount: 50_000,
        maxViewerCount: 60_000,
        orderingViewerCount: 12,
      },
      {
        id: "configured",
        tier: "background",
        viewerCount: 80,
        maxViewerCount: 100,
      },
      {
        id: "dgg-popular",
        tier: "background",
        viewerCount: 200,
        maxViewerCount: 300,
        orderingViewerCount: 90,
      },
    ]);

    expect(ordered.map((item) => item.id)).toEqual([
      "dgg-popular",
      "configured",
      "dgg-large-platform",
    ]);
    expect(ordered[2]?.viewerCount).toBe(50_000);
  });

  it("orders offline channels by their most recent end", () => {
    const ordered = sortOfflineDisplay([
      { id: "never", lastEndedAt: null },
      { id: "older", lastEndedAt: 10 },
      { id: "newer", lastEndedAt: 20 },
    ]);
    expect(ordered.map((item) => item.id)).toEqual(["newer", "older", "never"]);
  });
});
