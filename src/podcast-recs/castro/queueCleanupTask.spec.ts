import type { Logger } from "@micthiesen/mitools/logging";
import { describe, expect, it, vi } from "vitest";
import type { PodcastAccountClient, QueuedEpisode } from "../account.js";
import {
  CastroQueueCleanupTask,
  FREE_PREVIEW_DESCRIPTION_PREFIX,
  isFreePreviewEpisode,
} from "./queueCleanupTask.js";

function queued(description?: string): QueuedEpisode {
  return {
    showTitle: "Example Show",
    episodeTitle: "Example Episode",
    episodeGuid: "episode-guid",
    description,
  };
}

function accountWithQueue(queue: QueuedEpisode[]): PodcastAccountClient {
  return {
    name: "Castro",
    fetchSubscriptions: vi.fn(async () => ({ status: "ok" as const, value: [] })),
    fetchListenHistory: vi.fn(async () => ({ status: "ok" as const, value: [] })),
    fetchQueue: vi.fn(async () => ({ status: "ok" as const, value: queue })),
    searchPodcasts: vi.fn(async () => ({ status: "ok" as const, value: [] })),
    searchEpisodes: vi.fn(async () => ({ status: "ok" as const, value: [] })),
    enqueueEpisode: vi.fn(async () => "added" as const),
    dequeueEpisode: vi.fn(async () => "removed" as const),
    subscribeToShow: vi.fn(async () => "added" as const),
  };
}

const logger = { info: vi.fn() } as unknown as Logger;

describe("CastroQueueCleanupTask", () => {
  it("matches only descriptions that start with the Substack preview text", () => {
    expect(
      isFreePreviewEpisode(
        queued(`${FREE_PREVIEW_DESCRIPTION_PREFIX} of a paid subscriber post.`),
      ),
    ).toBe(true);
    expect(
      isFreePreviewEpisode(queued(`Intro. ${FREE_PREVIEW_DESCRIPTION_PREFIX}`)),
    ).toBe(false);
    expect(isFreePreviewEpisode(queued("this is a free preview"))).toBe(false);
    expect(isFreePreviewEpisode(queued())).toBe(false);
  });

  it("removes matching episodes and leaves the rest of the queue alone", async () => {
    const account = accountWithQueue([
      queued(`${FREE_PREVIEW_DESCRIPTION_PREFIX} from Substack.`),
      { ...queued("A normal episode."), episodeGuid: "normal-guid" },
    ]);
    const task = new CastroQueueCleanupTask(account, logger);

    await task.run();

    expect(account.dequeueEpisode).toHaveBeenCalledOnce();
    expect(account.dequeueEpisode).toHaveBeenCalledWith("episode-guid");
    expect(task.getLastRunSummary()).toBe("removed 1 free preview episode(s)");
    expect(task.schedule).toBe("0 * * * *");
  });

  it("fails rather than treating an unavailable queue as empty", async () => {
    const account = accountWithQueue([]);
    vi.mocked(account.fetchQueue).mockResolvedValue({
      status: "unavailable",
      reason: "Castro timed out",
    });
    const task = new CastroQueueCleanupTask(account, logger);

    await expect(task.run()).rejects.toThrow(
      "Castro queue unavailable: Castro timed out",
    );
    expect(account.dequeueEpisode).not.toHaveBeenCalled();
  });
});
