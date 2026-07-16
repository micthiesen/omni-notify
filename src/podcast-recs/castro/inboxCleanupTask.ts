import type { Logger } from "@micthiesen/mitools/logging";
import { ScheduledTask } from "@micthiesen/mitools/scheduling";
import {
  type InboxEpisode,
  type PodcastAccountClient,
  resolvePodcastAccount,
} from "../account.js";

export const FREE_PREVIEW_DESCRIPTION_PREFIX = "This is a free preview";

export function isFreePreviewEpisode(episode: InboxEpisode): boolean {
  return episode.description?.startsWith(FREE_PREVIEW_DESCRIPTION_PREFIX) ?? false;
}

export class CastroInboxCleanupTask extends ScheduledTask {
  public readonly name = "CastroInboxCleanup";
  public readonly schedule = "0 * * * *";
  public override readonly runOnStartup = false;
  // Drift off the exact top of the hour to avoid an obvious automated pattern.
  public override readonly jitterMs = 5 * 60 * 1000;

  private lastRunSummary?: string;

  public static create(parentLogger: Logger): CastroInboxCleanupTask | null {
    const logger = parentLogger.extend("CastroInboxCleanup");
    const account = resolvePodcastAccount(logger);
    if (!account) {
      parentLogger.info(
        "Castro inbox cleanup disabled: missing CASTRO_ACCESS_ID/CASTRO_SECRET_KEY",
      );
      return null;
    }
    return new CastroInboxCleanupTask(account, logger);
  }

  public constructor(
    private readonly account: PodcastAccountClient,
    private readonly logger: Logger,
  ) {
    super();
  }

  public async run(): Promise<void> {
    const inbox = await this.account.fetchInbox();
    if (inbox.status === "unavailable") {
      throw new Error(`Castro inbox unavailable: ${inbox.reason}`);
    }

    const previews = inbox.value.filter(isFreePreviewEpisode);
    let removed = 0;
    for (const episode of previews) {
      const result = await this.account.clearInboxEpisode(episode.clientEpisodeId);
      if (result === "removed") {
        removed++;
        this.logger.info(
          `Cleared free preview from Castro inbox: ${episode.showTitle} - ${episode.episodeTitle}`,
        );
        continue;
      }
      if (result !== "not_found") {
        throw new Error(
          `Could not clear Castro preview episode (${result}): ${episode.episodeTitle}`,
        );
      }
    }

    this.lastRunSummary = `cleared ${removed} free preview episode(s) from inbox`;
    this.logger.info(`Castro inbox cleanup finished: ${this.lastRunSummary}`);
  }

  public getLastRunSummary(): string | undefined {
    return this.lastRunSummary;
  }
}
