import type { Logger } from "@micthiesen/mitools/logging";
import { notify } from "@micthiesen/mitools/pushover";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ViewerMetricsData } from "./types.js";
import { ViewerMetricsService } from "./ViewerMetricsService.js";

vi.mock("@micthiesen/mitools/pushover", () => ({
  notify: vi.fn(async () => {}),
}));

const store = vi.hoisted(() => new Map<string, ViewerMetricsData>());
vi.mock("./persistence.js", () => ({
  getViewerMetrics: (streamerId: string): ViewerMetricsData =>
    store.get(streamerId) ?? {
      streamerId,
      dailyBuckets: [],
      allTimeMax: 0,
      allTimeMaxTimestamp: 0,
    },
  upsertViewerMetrics: (metrics: ViewerMetricsData): void => {
    store.set(metrics.streamerId, metrics);
  },
}));

const noopLogger = {
  extend: () => noopLogger,
  debug: () => {},
  info: () => {},
  warn: () => {},
  error: () => {},
} as unknown as Logger;

const urlFields = { url: "https://example.com", url_title: "Watch" };

function makeService(token?: string): ViewerMetricsService {
  return new ViewerMetricsService(() => token, noopLogger);
}

describe("ViewerMetricsService notifications", () => {
  beforeEach(() => {
    store.clear();
    vi.mocked(notify).mockClear();
  });

  // The service has no liveNotifications gate by design: viewer-record
  // notifications fire for every streamer, including those muted via
  // channels.json (liveNotifications: false). Muting is applied only to the
  // went-live/title-change/went-offline paths in task.ts (see
  // notificationPolicy.spec.ts); nothing streamer-specific reaches this
  // service except the Pushover token.
  it("notifies on a confirmed record with no mute path (records fire for muted streamers too)", async () => {
    const service = makeService("tok-live");
    const base = { streamerId: "muted", displayName: "Muted", urlFields };

    // Climb to a new peak, then drop >5% to confirm it.
    await service.recordViewerCount({ ...base, viewerCount: 100 });
    expect(notify).not.toHaveBeenCalled();
    await service.recordViewerCount({ ...base, viewerCount: 90 });

    expect(notify).toHaveBeenCalledTimes(1);
    expect(notify).toHaveBeenCalledWith(
      expect.objectContaining({
        title: expect.stringContaining("Muted"),
        token: "tok-live",
        ...urlFields,
      }),
    );
  });

  it("notifies on flushPendingPeaks (offline flush) with no mute path", async () => {
    const service = makeService(undefined);
    const base = { streamerId: "muted", displayName: "Muted", urlFields };

    await service.recordViewerCount({ ...base, viewerCount: 100 });
    expect(notify).not.toHaveBeenCalled();
    await service.flushPendingPeaks(base);

    expect(notify).toHaveBeenCalledTimes(1);
    expect(notify).toHaveBeenCalledWith(
      expect.objectContaining({ title: expect.stringContaining("Muted") }),
    );
  });

  it("does not notify while a peak is still climbing", async () => {
    const service = makeService(undefined);
    const base = { streamerId: "s", displayName: "S", urlFields };

    await service.recordViewerCount({ ...base, viewerCount: 100 });
    await service.recordViewerCount({ ...base, viewerCount: 150 });
    await service.recordViewerCount({ ...base, viewerCount: 149 });

    expect(notify).not.toHaveBeenCalled();
  });
});
