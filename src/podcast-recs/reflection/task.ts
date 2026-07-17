import type { Logger } from "@micthiesen/mitools/logging";
import { ScheduledTask } from "@micthiesen/mitools/scheduling";
import { getPodcastTasteReflectionModel } from "../../ai/registry.js";
import config from "../../utils/config.js";
import { resolvePodcastAccount } from "../account.js";
import { getAllPodcastRecommendations } from "../persistence.js";
import { runPodcastTasteReflection } from "./reflection.js";

export class PodcastTasteReflectionTask extends ScheduledTask {
  public readonly name = "PodcastTasteReflection";
  public readonly schedule = config.PODCAST_TASTE_REFLECTION_SCHEDULE;
  public override readonly runOnStartup = false;
  // Fire a few minutes off the scheduled instant so we don't hit Castro at a
  // predictable time (well-behaved-client rule for the private sync API).
  public override readonly jitterMs = 5 * 60 * 1000;

  private lastRunSummary?: string;
  private readonly logger: Logger;

  public static create(parentLogger: Logger): PodcastTasteReflectionTask | null {
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
      parentLogger.info(
        `Podcast taste reflection disabled: missing ${missing.join(", ")}`,
      );
      return null;
    }
    return new PodcastTasteReflectionTask(parentLogger);
  }

  private constructor(logger: Logger) {
    super();
    this.logger = logger.extend("PodcastTasteReflection");
  }

  public async run(): Promise<void> {
    const account = resolvePodcastAccount(this.logger);
    if (!account) {
      this.lastRunSummary = "skipped: no podcast account client";
      this.logger.warn("Podcast taste reflection skipped: no account client");
      return;
    }

    // Full window (the client caps at 180 days): unlike outcome sync, this is
    // the deep evidence-gathering read, and it runs only weekly.
    const history = await account.fetchListenHistory();
    if (history.status === "unavailable") {
      this.lastRunSummary = `skipped: ${history.reason}`;
      this.logger.warn(`Podcast taste reflection skipped: ${history.reason}`);
      return;
    }

    const { model, modelId } = getPodcastTasteReflectionModel();
    const result = await runPodcastTasteReflection({
      listened: history.value,
      recommendations: getAllPodcastRecommendations(),
      model,
      modelId,
    });
    if (result.status === "created") {
      this.lastRunSummary = `profile v${result.profile.version}: ${result.profile.evidenceCount} evidence items, ${result.rejectedClaims} unsupported claims removed`;
    } else if (result.status === "unchanged") {
      this.lastRunSummary = `unchanged: profile v${result.profile.version}, no model call`;
    } else {
      this.lastRunSummary = "no listen or recommendation evidence";
    }
    this.logger.info(`Podcast taste reflection finished: ${this.lastRunSummary}`);
  }

  public getLastRunSummary(): string | undefined {
    return this.lastRunSummary;
  }
}
