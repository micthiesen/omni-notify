import type { Logger } from "@micthiesen/mitools/logging";
import { ScheduledTask } from "@micthiesen/mitools/scheduling";
import {
  type PodcastAccountClient,
  type QueuedEpisode,
  resolvePodcastAccount,
} from "../account.js";

export const FREE_PREVIEW_DESCRIPTION_PREFIX = "This is a free preview";

export function isFreePreviewEpisode(episode: QueuedEpisode): boolean {
  return episode.description?.startsWith(FREE_PREVIEW_DESCRIPTION_PREFIX) ?? false;
}

export class CastroQueueCleanupTask extends ScheduledTask {
  public readonly name = "CastroQueueCleanup";
  public readonly schedule = "0 * * * *";
  public override readonly runOnStartup = false;
  // Drift off the exact top of the hour — an on-the-dot hourly hit is the most
  // obvious "this is a bot" signature.
  public override readonly jitterMs = 5 * 60 * 1000;

  private lastRunSummary?: string;

  public static create(parentLogger: Logger): CastroQueueCleanupTask | null {
    const logger = parentLogger.extend("CastroQueueCleanup");
    const account = resolvePodcastAccount(logger);
    if (!account) {
      parentLogger.info(
        "Castro queue cleanup disabled: missing CASTRO_ACCESS_ID/CASTRO_SECRET_KEY",
      );
      return null;
    }
    return new CastroQueueCleanupTask(account, logger);
  }

  public constructor(
    private readonly account: PodcastAccountClient,
    private readonly logger: Logger,
  ) {
    super();
  }

  public async run(): Promise<void> {
    const queue = await this.account.fetchQueue();
    if (queue.status === "unavailable") {
      throw new Error(`Castro queue unavailable: ${queue.reason}`);
    }

    const previews = queue.value.filter(isFreePreviewEpisode);
    let removed = 0;
    for (const episode of previews) {
      if (!episode.episodeGuid) continue;
      const result = await this.account.dequeueEpisode(episode.episodeGuid);
      if (result === "removed") {
        removed++;
        this.logger.info(
          `Removed free preview from Castro queue: ${episode.showTitle} - ${episode.episodeTitle}`,
        );
        continue;
      }
      if (result !== "not_found") {
        throw new Error(
          `Could not remove Castro preview episode (${result}): ${episode.episodeTitle}`,
        );
      }
    }

    this.lastRunSummary = `removed ${removed} free preview episode(s)`;
    this.logger.info(`Castro queue cleanup finished: ${this.lastRunSummary}`);
  }

  public getLastRunSummary(): string | undefined {
    return this.lastRunSummary;
  }
}
