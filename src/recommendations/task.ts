import { LogFile } from "@micthiesen/mitools/logfile";
import type { Logger } from "@micthiesen/mitools/logging";
import { logTimestamp } from "@micthiesen/mitools/markdown";
import { ScheduledTask } from "@micthiesen/mitools/scheduling";
import config from "../utils/config.js";
import { runRecommendationPipeline } from "./pipeline.js";

export class RecommendationTask extends ScheduledTask {
  public readonly name = "Recommendations";
  public readonly schedule = config.RECS_SCHEDULE;
  public override readonly runOnStartup = false;

  private logger: Logger;
  private lastRunSummary?: string;

  public static create(parentLogger: Logger): RecommendationTask | null {
    const missing = [
      ["TMDB_API_KEY", config.TMDB_API_KEY],
      ["TAVILY_API_KEY", config.TAVILY_API_KEY],
      ...requiredModelCredentials(),
      ["PLEX_URL", config.PLEX_URL],
      ["PLEX_TOKEN", config.PLEX_TOKEN],
      ["RADARR_URL", config.RADARR_URL],
      ["RADARR_API_KEY", config.RADARR_API_KEY],
      ["RADARR_ROOT_FOLDER_PATH", config.RADARR_ROOT_FOLDER_PATH],
      ["RADARR_QUALITY_PROFILE_ID", config.RADARR_QUALITY_PROFILE_ID],
      ["SONARR_URL", config.SONARR_URL],
      ["SONARR_API_KEY", config.SONARR_API_KEY],
      ["SONARR_ROOT_FOLDER_PATH", config.SONARR_ROOT_FOLDER_PATH],
      ["SONARR_QUALITY_PROFILE_ID", config.SONARR_QUALITY_PROFILE_ID],
    ]
      .filter(([, value]) => !value)
      .map(([name]) => name);
    if (missing.length > 0) {
      parentLogger.info(`Recommendations disabled: missing ${missing.join(", ")}`);
      return null;
    }
    return new RecommendationTask(parentLogger);
  }

  private constructor(parentLogger: Logger) {
    super();
    this.logger = parentLogger.extend("RecsTask");
  }

  public async run(): Promise<void> {
    const logFile = config.LOGS_PATH
      ? new LogFile(
          `${config.LOGS_PATH}/recommendations/${logTimestamp()}.md`,
          "overwrite",
        )
      : undefined;

    const summary = await runRecommendationPipeline(this.logger, logFile);
    this.lastRunSummary = summary;
    this.logger.info(`Recommendation run finished: ${summary}`);
  }

  /** Consumed by the task-run tracking registry. */
  public getLastRunSummary(): string | undefined {
    return this.lastRunSummary;
  }
}

function requiredModelCredentials(): [string, unknown][] {
  const modelIds = [
    config.RECS_SHORTLIST_MODEL ?? "openai:gpt-5-mini",
    config.RECS_SELECTION_MODEL ?? "openai:gpt-5",
  ];
  const providers = new Set(modelIds.map((id) => id.split(":", 1)[0]));
  const credentials: [string, unknown][] = [];
  if (providers.has("openai")) {
    credentials.push(["OPENAI_API_KEY", config.OPENAI_API_KEY]);
  }
  if (providers.has("anthropic")) {
    credentials.push(["ANTHROPIC_API_KEY", config.ANTHROPIC_API_KEY]);
  }
  if (providers.has("google")) {
    credentials.push([
      "GOOGLE_GENERATIVE_AI_API_KEY",
      config.GOOGLE_GENERATIVE_AI_API_KEY,
    ]);
  }
  return credentials;
}
