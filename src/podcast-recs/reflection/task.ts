import type { NamedLogger as Logger } from "@micthiesen/mitools/logging";
import type { ScheduledTask } from "@micthiesen/mitools/scheduling";
import { Effect } from "effect";
import { getPodcastTasteReflectionModel } from "../../ai/registry.js";
import config from "../../utils/config.js";
import type { TaskServices } from "../../task-runs/registry.js";
import { resolvePodcastAccountEffect } from "../account.js";
import { getAllPodcastRecommendations } from "../persistence.js";
import { runPodcastTasteReflectionEffect } from "./reflection.js";

export class PodcastTasteReflectionTask implements ScheduledTask<
  unknown,
  TaskServices
> {
  public readonly name = "PodcastTasteReflection";
  public readonly displayName = "Podcast Taste Reflection";
  public readonly schedule = config.PODCAST_TASTE_REFLECTION_SCHEDULE;
  public readonly runOnStartup = false;
  // Fire a few minutes off the scheduled instant so we don't hit Castro at a
  // predictable time (well-behaved-client rule for the private sync API).
  public readonly jitterMs = 5 * 60 * 1000;

  private lastRunSummary?: string;
  private readonly logger: Logger;

  public static create(parentLogger: Logger) {
    return Effect.gen(function* () {
      const modelId = config.PODCAST_TASTE_REFLECTION_MODEL ?? "openai:gpt-5.6-luna";
      const provider = modelId.split(":", 1)[0];
      const credential =
        provider === "openai"
          ? config.OPENAI_API_KEY
          : provider === "anthropic"
            ? config.ANTHROPIC_API_KEY
            : provider === "google"
              ? config.GOOGLE_GENERATIVE_AI_API_KEY
              : undefined;
      const missing = [
        !config.PODCAST_TASTE_PATH && "PODCAST_TASTE_PATH",
        !config.CASTRO_ACCESS_ID && "CASTRO_ACCESS_ID",
        !config.CASTRO_SECRET_KEY && "CASTRO_SECRET_KEY",
        !credential && `${provider.toUpperCase()} model credential`,
      ].filter((name): name is string => Boolean(name));
      if (missing.length > 0) {
        yield* parentLogger.info(
          `Podcast taste reflection disabled: missing ${missing.join(", ")}`,
        );
        return null;
      }
      return new PodcastTasteReflectionTask(parentLogger);
    });
  }

  private constructor(logger: Logger) {
    this.logger = logger.extend("PodcastTasteReflection");
  }

  public readonly run = Effect.gen({ self: this }, function* () {
    const account = yield* resolvePodcastAccountEffect(this.logger);
    if (!account) {
      this.lastRunSummary = "skipped: no podcast account client";
      yield* this.logger.warn("Podcast taste reflection skipped: no account client");
      return;
    }

    // Full window (the client caps at 180 days): unlike outcome sync, this is
    // the deep evidence-gathering read, and it runs only weekly.
    const history = yield* account.fetchListenHistory();
    if (history.status === "unavailable") {
      this.lastRunSummary = `skipped: ${history.reason}`;
      yield* this.logger.warn(`Podcast taste reflection skipped: ${history.reason}`);
      return;
    }

    const { model, modelId } = getPodcastTasteReflectionModel();
    const result = yield* runPodcastTasteReflectionEffect({
      listened: history.value,
      recommendations: yield* getAllPodcastRecommendations(),
      model,
      modelId,
    });
    if (result.status === "created" && result.profile) {
      this.lastRunSummary = `profile v${result.profile.version}: ${result.profile.evidenceCount} evidence items, ${result.rejectedClaims} unsupported claims removed`;
    } else if (result.status === "unchanged" && result.profile) {
      this.lastRunSummary = `unchanged: profile v${result.profile.version}, no model call`;
    } else {
      this.lastRunSummary = "no listen or recommendation evidence";
    }
    yield* this.logger.info(
      `Podcast taste reflection finished: ${this.lastRunSummary}`,
    );
  });

  public getLastRunSummary(): string | undefined {
    return this.lastRunSummary;
  }
}
