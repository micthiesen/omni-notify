import type { Logger } from "@micthiesen/mitools/logging";
import { describe, expect, it, vi } from "vitest";
import type { InboxEpisode, PodcastAccountClient } from "../account.js";
import {
  CastroInboxCleanupTask,
  FREE_PREVIEW_DESCRIPTION_PREFIX,
  isFreePreviewEpisode,
} from "./inboxCleanupTask.js";

function inboxEpisode(description?: string): InboxEpisode {
  return {
    clientEpisodeId: "11111111-1111-4111-8111-111111111111",
    showTitle: "Example Show",
    episodeTitle: "Example Episode",
    episodeGuid: "episode-guid",
    description,
  };
}

function accountWithInbox(inbox: InboxEpisode[]): PodcastAccountClient {
  return {
    name: "Castro",
    fetchSubscriptions: vi.fn(async () => ({ status: "ok" as const, value: [] })),
    fetchListenHistory: vi.fn(async () => ({ status: "ok" as const, value: [] })),
    fetchQueue: vi.fn(async () => ({ status: "ok" as const, value: [] })),
    fetchInbox: vi.fn(async () => ({ status: "ok" as const, value: inbox })),
    searchPodcasts: vi.fn(async () => ({ status: "ok" as const, value: [] })),
    searchEpisodes: vi.fn(async () => ({ status: "ok" as const, value: [] })),
    enqueueEpisode: vi.fn(async () => "added" as const),
    dequeueEpisode: vi.fn(async () => "removed" as const),
    clearInboxEpisode: vi.fn(async () => "removed" as const),
    subscribeToShow: vi.fn(async () => "added" as const),
  };
}

const logger = { info: vi.fn() } as unknown as Logger;

describe("CastroInboxCleanupTask", () => {
  it("matches only descriptions that start with the Substack preview text", () => {
    expect(
      isFreePreviewEpisode(
        inboxEpisode(`${FREE_PREVIEW_DESCRIPTION_PREFIX} of a paid post.`),
      ),
    ).toBe(true);
    expect(
      isFreePreviewEpisode(inboxEpisode(`Intro. ${FREE_PREVIEW_DESCRIPTION_PREFIX}`)),
    ).toBe(false);
    expect(isFreePreviewEpisode(inboxEpisode("this is a free preview"))).toBe(false);
    expect(isFreePreviewEpisode(inboxEpisode())).toBe(false);
  });

  it("clears matching Inbox entries without touching the Queue", async () => {
    const account = accountWithInbox([
      inboxEpisode(`${FREE_PREVIEW_DESCRIPTION_PREFIX} from Substack.`),
      {
        ...inboxEpisode("A normal episode."),
        clientEpisodeId: "22222222-2222-4222-8222-222222222222",
      },
    ]);
    const task = new CastroInboxCleanupTask(account, logger);

    await task.run();

    expect(account.clearInboxEpisode).toHaveBeenCalledOnce();
    expect(account.clearInboxEpisode).toHaveBeenCalledWith(
      "11111111-1111-4111-8111-111111111111",
    );
    expect(account.dequeueEpisode).not.toHaveBeenCalled();
    expect(account.fetchQueue).not.toHaveBeenCalled();
    expect(task.getLastRunSummary()).toBe(
      "cleared 1 free preview episode(s) from inbox",
    );
    expect(task.schedule).toBe("0 * * * *");
  });

  it("fails rather than treating an unavailable Inbox as empty", async () => {
    const account = accountWithInbox([]);
    vi.mocked(account.fetchInbox).mockResolvedValue({
      status: "unavailable",
      reason: "Castro timed out",
    });
    const task = new CastroInboxCleanupTask(account, logger);

    await expect(task.run()).rejects.toThrow(
      "Castro inbox unavailable: Castro timed out",
    );
    expect(account.clearInboxEpisode).not.toHaveBeenCalled();
  });
});
