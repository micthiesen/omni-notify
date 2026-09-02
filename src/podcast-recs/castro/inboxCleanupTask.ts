import type { Logger } from "@micthiesen/mitools/logging";
import { ScheduledTask } from "@micthiesen/mitools/scheduling";
import { Data, Effect } from "effect";
import config from "../../utils/config.js";
import {
  type InboxEpisode,
  type PodcastAccountClient,
  resolvePodcastAccountEffect,
} from "../account.js";

export const FREE_PREVIEW_DESCRIPTION_PREFIX = "This is a free preview";

export function isFreePreviewEpisode(episode: InboxEpisode): boolean {
  return episode.description?.startsWith(FREE_PREVIEW_DESCRIPTION_PREFIX) ?? false;
}

class CastroInboxCleanupError extends Data.TaggedError("CastroInboxCleanupError")<{
  readonly operation: string;
  readonly cause: unknown;
}> {
  public override get message(): string {
    const detail =
      this.cause instanceof Error ? this.cause.message : String(this.cause);
    return `${this.operation}: ${detail}`;
  }
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
    if (!config.CASTRO_ACCESS_ID || !config.CASTRO_SECRET_KEY) {
      parentLogger.info(
        "Castro inbox cleanup disabled: missing CASTRO_ACCESS_ID/CASTRO_SECRET_KEY",
      );
      return null;
    }
    return new CastroInboxCleanupTask(logger);
  }

  public constructor(
    private readonly logger: Logger,
    private readonly accountOverride?: PodcastAccountClient,
  ) {
    super();
  }

  public async run(): Promise<void> {
    return Effect.runPromise(this.runEffect());
  }

  private runEffect(): Effect.Effect<void, CastroInboxCleanupError> {
    return Effect.gen({ self: this }, function* () {
      const account =
        this.accountOverride ?? (yield* resolvePodcastAccountEffect(this.logger));
      if (!account) {
        return yield* Effect.fail(
          new CastroInboxCleanupError({
            operation: "create account client",
            cause: new Error("Castro account is not configured"),
          }),
        );
      }
      const inbox = yield* account.fetchInbox();
      if (inbox.status === "unavailable") {
        return yield* Effect.fail(
          new CastroInboxCleanupError({
            operation: "fetch inbox",
            cause: new Error(`Castro inbox unavailable: ${inbox.reason}`),
          }),
        );
      }

      const previews = inbox.value.filter(isFreePreviewEpisode);
      const results = yield* Effect.forEach(previews, (episode) =>
        Effect.gen({ self: this }, function* () {
          const result = yield* account.clearInboxEpisode(episode.clientEpisodeId);
          if (result === "removed") {
            this.logger.info(
              `Cleared free preview from Castro inbox: ${episode.showTitle} - ${episode.episodeTitle}`,
            );
            return true;
          }
          if (result !== "not_found") {
            return yield* Effect.fail(
              new CastroInboxCleanupError({
                operation: "clear inbox episode",
                cause: new Error(
                  `Could not clear Castro preview episode (${result}): ${episode.episodeTitle}`,
                ),
              }),
            );
          }
          return false;
        }),
      );

      const removed = results.filter(Boolean).length;
      this.lastRunSummary = `cleared ${removed} free preview episode(s) from inbox`;
      this.logger.info(`Castro inbox cleanup finished: ${this.lastRunSummary}`);
    });
  }

  public getLastRunSummary(): string | undefined {
    return this.lastRunSummary;
  }
}
