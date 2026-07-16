import { LogFile } from "@micthiesen/mitools/logfile";
import type { Logger } from "@micthiesen/mitools/logging";
import { logTimestamp } from "@micthiesen/mitools/markdown";
import { ScheduledTask } from "@micthiesen/mitools/scheduling";
import config from "../utils/config.js";
import { MAX_PODCAST_RECOMMENDATIONS_PER_RUN, runPodcastPipeline } from "./pipeline.js";

export interface PodcastRecommendationManualRunInput {
  maxRecommendations: number;
}

export class PodcastRecommendationTask extends ScheduledTask {
  public readonly name = "PodcastRecs";
  public readonly schedule = config.PODCAST_RECS_SCHEDULE;
  public override readonly runOnStartup = false;
  // Fire a few minutes off the scheduled instant so we don't hit Castro at a
  // predictable round time.
  public override readonly jitterMs = 5 * 60 * 1000;

  private logger: Logger;
  private lastRunSummary?: string;

  public static create(parentLogger: Logger): PodcastRecommendationTask | null {
    const missing = [
      // The taste seed doubles as the feature flag: the task stays disabled
      // until a profile file is mounted (see CLAUDE.md / README).
      ["PODCAST_TASTE_PATH", config.PODCAST_TASTE_PATH],
      ["TAVILY_API_KEY", config.TAVILY_API_KEY],
      ...requiredModelCredentials(),
    ]
      .filter(([, value]) => !value)
      .map(([name]) => name);
    if (missing.length > 0) {
      parentLogger.info(
        `Podcast recommendations disabled: missing ${missing.join(", ")}`,
      );
      return null;
    }
    return new PodcastRecommendationTask(parentLogger);
  }

  private constructor(parentLogger: Logger) {
    super();
    this.logger = parentLogger.extend("PodcastRecsTask");
  }

  public async run(): Promise<void> {
    await this.runPipeline(2);
  }

  public async runManual(input: unknown): Promise<void> {
    await this.runPipeline(parseMaxRecommendations(input));
  }

  private async runPipeline(maxRecommendations: number): Promise<void> {
    const logFile = config.LOGS_PATH
      ? new LogFile(
          `${config.LOGS_PATH}/podcast-recs/${logTimestamp()}.md`,
          "overwrite",
        )
      : undefined;

    this.logger.info(
      `Podcast recommendation run requested up to ${maxRecommendations} episode(s)`,
    );
    const summary = await runPodcastPipeline(this.logger, logFile, {
      maxRecommendations,
    });
    this.lastRunSummary = summary;
    this.logger.info(`Podcast recommendation run finished: ${summary}`);
  }

  /** Consumed by the task-run tracking registry. */
  public getLastRunSummary(): string | undefined {
    return this.lastRunSummary;
  }
}

function parseMaxRecommendations(input: unknown): number {
  const maxRecommendations = (
    input as Partial<PodcastRecommendationManualRunInput> | null
  )?.maxRecommendations;
  if (
    !Number.isInteger(maxRecommendations) ||
    maxRecommendations === undefined ||
    maxRecommendations < 1 ||
    maxRecommendations > MAX_PODCAST_RECOMMENDATIONS_PER_RUN
  ) {
    throw new RangeError(
      `maxRecommendations must be an integer from 1 to ${MAX_PODCAST_RECOMMENDATIONS_PER_RUN}`,
    );
  }
  return maxRecommendations;
}

function requiredModelCredentials(): [string, unknown][] {
  const modelIds = [
    config.RECS_SHORTLIST_MODEL ?? "openai:gpt-5.6-luna",
    config.RECS_SELECTION_MODEL ?? "openai:gpt-5.6",
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
