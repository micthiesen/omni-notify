import { LogFile } from "@micthiesen/mitools/logfile";
import type { Logger } from "@micthiesen/mitools/logging";
import { logTimestamp } from "@micthiesen/mitools/markdown";
import { ScheduledTask } from "@micthiesen/mitools/scheduling";
import config from "../utils/config.js";
import { runRecommendationPipeline } from "./pipeline.js";

export class RecommendationTask extends ScheduledTask {
  public readonly name = "Recommendations";
  public readonly schedule = config.RECS_SCHEDULE;

  private logger: Logger;
  private lastRunSummary?: string;

  public static create(parentLogger: Logger): RecommendationTask | null {
    const missing = [
      ["TMDB_API_KEY", config.TMDB_API_KEY],
      ["OPENAI_API_KEY", config.OPENAI_API_KEY],
      ["TAVILY_API_KEY", config.TAVILY_API_KEY],
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
