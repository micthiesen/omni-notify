import type { NamedLogger } from "@micthiesen/mitools/logging";
import type { ScheduledTask } from "@micthiesen/mitools/scheduling";
import { Effect } from "effect";
import type { TaskServices } from "../task-runs/registry.js";
import config from "../utils/config.js";
import { createArrClient } from "./client.js";
import { runRecovery } from "./service.js";
import { verifyDownloadRemoved } from "./filesystem.js";
import type { ArrClient, ArrKind } from "./types.js";

export class ArrRecoveryTask implements ScheduledTask<unknown, TaskServices> {
  public readonly name = "ArrRecovery";
  public readonly displayName = "Sonarr / Radarr Recovery";
  public readonly schedule = "0 */5 * * * *";
  public readonly runOnStartup = true;
  private lastRunSummary?: string;

  public static create(parentLogger: NamedLogger) {
    return Effect.gen(function* () {
      if (!config.ARR_RECOVERY_ENABLED) return null;
      const clients: ArrClient[] = [];
      for (const [kind, url, apiKey] of [
        ["sonarr", config.SONARR_URL, config.SONARR_API_KEY],
        ["radarr", config.RADARR_URL, config.RADARR_API_KEY],
      ] as const) {
        if (url && apiKey) {
          const client = createArrClient({ kind: kind as ArrKind, url, apiKey });
          if (config.ARR_RECOVERY_LOCAL_FILES)
            client.verifyRemoved = verifyDownloadRemoved;
          clients.push(client);
        }
      }
      if (clients.length === 0) return null;
      if (
        !config.OPENAI_API_KEY ||
        !config.PUSHOVER_USER ||
        !config.PUSHOVER_RECS_TOKEN
      ) {
        yield* parentLogger.warn(
          "Arr recovery disabled: requires OpenAI and Pushover credentials",
        );
        return null;
      }
      return new ArrRecoveryTask(parentLogger.extend("ArrRecovery"), clients);
    });
  }
  public constructor(
    private readonly logger: NamedLogger,
    private readonly clients: ArrClient[],
  ) {}
  public readonly run = Effect.gen({ self: this }, function* () {
    this.lastRunSummary = undefined;
    this.lastRunSummary = yield* runRecovery(this.clients, this.logger);
  });
  public getLastRunSummary(): string | undefined {
    return this.lastRunSummary;
  }
}
