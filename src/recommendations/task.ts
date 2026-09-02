import { LogFile } from "@micthiesen/mitools/logfile";
import type { NamedLogger } from "@micthiesen/mitools/logging";
import { logTimestamp } from "@micthiesen/mitools/markdown";
import type { ScheduledTask } from "@micthiesen/mitools/scheduling";
import { Clock, Effect, Schema } from "effect";
import config from "../utils/config.js";
import type { TaskServices } from "../task-runs/registry.js";
import { RecommendationInputError } from "./effect.js";
import {
  MAX_RECOMMENDATIONS_PER_RUN,
  runRecommendationPipelineEffect,
} from "./pipeline.js";

export interface RecommendationManualRunInput {
  maxRecommendations: number;
}

export class MediaRecommendationTask implements ScheduledTask<unknown, TaskServices> {
  public readonly name = "Recommendations";
  public readonly displayName = "Media Recommendations";
  public readonly schedule = config.RECS_SCHEDULE;
  public readonly runOnStartup = false;

  private logger: NamedLogger;
  private lastRunSummary?: string;

  public static create(parentLogger: NamedLogger) {
    return Effect.gen(function* () {
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
        yield* parentLogger.info(
          `Recommendations disabled: missing ${missing.join(", ")}`,
        );
        return null;
      }
      return new MediaRecommendationTask(parentLogger);
    });
  }

  private constructor(parentLogger: NamedLogger) {
    this.logger = parentLogger.extend("RecsTask");
  }

  public readonly run = this.runPipelineEffect(1);

  public runManual(input: unknown) {
    return decodeManualInput(input).pipe(
      Effect.flatMap(({ maxRecommendations }) =>
        this.runPipelineEffect(maxRecommendations),
      ),
    );
  }

  private runPipelineEffect(maxRecommendations: number) {
    return Effect.gen({ self: this }, function* () {
      const now = yield* Clock.currentTimeMillis;
      const logFile = config.LOGS_PATH
        ? yield* LogFile.make(
            `${config.LOGS_PATH}/recommendations/${logTimestamp(new Date(now))}.md`,
            "overwrite",
          )
        : undefined;

      yield* this.logger.info(
        `Recommendation run requested up to ${maxRecommendations} item(s)`,
      );
      const summary = yield* runRecommendationPipelineEffect(this.logger, logFile, {
        maxRecommendations,
      });
      this.lastRunSummary = summary;
      yield* this.logger.info(`Recommendation run finished: ${summary}`);
    });
  }

  /** Consumed by the task-run tracking registry. */
  public getLastRunSummary(): string | undefined {
    return this.lastRunSummary;
  }
}

const recommendationManualInputSchema = Schema.Struct({
  maxRecommendations: Schema.Number,
});

function decodeManualInput(
  input: unknown,
): Effect.Effect<RecommendationManualRunInput, RecommendationInputError> {
  return Schema.decodeUnknownEffect(recommendationManualInputSchema)(input).pipe(
    Effect.filterOrFail(
      ({ maxRecommendations }) =>
        Number.isInteger(maxRecommendations) &&
        maxRecommendations >= 1 &&
        maxRecommendations <= MAX_RECOMMENDATIONS_PER_RUN,
      () =>
        new RecommendationInputError({
          message: `maxRecommendations must be an integer from 1 to ${MAX_RECOMMENDATIONS_PER_RUN}`,
        }),
    ),
    Effect.mapError((cause) =>
      cause instanceof RecommendationInputError
        ? cause
        : new RecommendationInputError({
            message: `maxRecommendations must be an integer from 1 to ${MAX_RECOMMENDATIONS_PER_RUN}`,
          }),
    ),
  );
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
