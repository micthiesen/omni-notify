import { beforeEach, describe, expect, it, vi } from "vitest";

const plex = vi.hoisted(() => ({
  fetchWatchHistory: vi.fn(),
  fetchInProgress: vi.fn(),
  fetchLibraryIndex: vi.fn(),
}));

vi.mock("../utils/config.js", () => ({
  default: { PLEX_URL: "http://plex", PLEX_TOKEN: "token" },
}));
vi.mock("./plex/client.js", () => ({ createPlexClient: () => plex }));

import {
  fetchInProgress,
  fetchLibraryIndex,
  fetchWatchHistory,
} from "./mediaLibrary.js";

describe("Plex media library bridge", () => {
  beforeEach(() => vi.resetAllMocks());

  it("wraps successful Plex responses", async () => {
    plex.fetchWatchHistory.mockResolvedValue([{ guid: "plex://movie/1" }]);
    plex.fetchInProgress.mockResolvedValue([]);
    plex.fetchLibraryIndex.mockResolvedValue([]);

    await expect(fetchWatchHistory()).resolves.toEqual({
      status: "ok",
      value: [{ guid: "plex://movie/1" }],
    });
    await expect(fetchInProgress()).resolves.toEqual({ status: "ok", value: [] });
    await expect(fetchLibraryIndex()).resolves.toEqual({ status: "ok", value: [] });
  });

  it("reports Plex failures as unavailable instead of empty state", async () => {
    plex.fetchWatchHistory.mockRejectedValue(new Error("Plex timed out"));

    await expect(fetchWatchHistory()).resolves.toEqual({
      status: "unavailable",
      reason: "Plex timed out",
    });
  });
});
