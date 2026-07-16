import type { Logger } from "@micthiesen/mitools/logging";
import { ScheduledTask } from "@micthiesen/mitools/scheduling";
import PQueue from "p-queue";
import { getTasteReflectionModel } from "../../ai/registry.js";
import config from "../../utils/config.js";
import { completedWatches } from "../history.js";
import { RESOLUTION_CONFIDENCE_THRESHOLD, resolveIdentity } from "../identity.js";
import { fetchWatchHistory } from "../mediaLibrary.js";
import { getAllRecommendations } from "../persistence.js";
import { fetchTitleDetails } from "../tmdb/client.js";
import type { CanonicalId, WatchedItem } from "../types.js";
import { runTasteReflection } from "./reflection.js";
import type { CanonicalWatchObservation } from "./types.js";

const MAX_WATCH_EVIDENCE = 160;

export class TasteReflectionTask extends ScheduledTask {
  public readonly name = "TasteReflection";
  public readonly schedule = config.TASTE_REFLECTION_SCHEDULE;
  public override readonly runOnStartup = false;

  private lastRunSummary?: string;
  private readonly logger: Logger;

  public static create(parentLogger: Logger): TasteReflectionTask | null {
    const modelId = config.TASTE_REFLECTION_MODEL ?? "openai:gpt-5-mini";
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
      !config.TMDB_API_KEY && "TMDB_API_KEY",
      !config.PLEX_URL && "PLEX_URL",
      !config.PLEX_TOKEN && "PLEX_TOKEN",
      !credential && `${provider.toUpperCase()} model credential`,
    ].filter((name): name is string => Boolean(name));
    if (missing.length > 0) {
      parentLogger.info(`Taste reflection disabled: missing ${missing.join(", ")}`);
      return null;
    }
    return new TasteReflectionTask(parentLogger);
  }

  private constructor(logger: Logger) {
    super();
    this.logger = logger.extend("TasteReflection");
  }

  public async run(): Promise<void> {
    const history = await fetchWatchHistory();
    if (history.status === "unavailable") {
      this.lastRunSummary = `skipped: ${history.reason}`;
      this.logger.warn(`Taste reflection skipped: ${history.reason}`);
      return;
    }

    const watched = await buildCanonicalWatchEvidence(history.value, this.logger);
    const { model, modelId } = getTasteReflectionModel();
    const result = await runTasteReflection({
      watched,
      recommendations: getAllRecommendations(),
      model,
      modelId,
    });
    if (result.status === "created") {
      this.lastRunSummary = `profile v${result.profile.version}: ${result.profile.evidenceCount} evidence items, ${result.rejectedClaims} unsupported claims removed`;
    } else if (result.status === "unchanged") {
      this.lastRunSummary = `unchanged: profile v${result.profile.version}, no model call`;
    } else {
      this.lastRunSummary = "no completed watch or recommendation evidence";
    }
    this.logger.info(`Taste reflection finished: ${this.lastRunSummary}`);
  }

  public getLastRunSummary(): string | undefined {
    return this.lastRunSummary;
  }
}

async function buildCanonicalWatchEvidence(
  history: WatchedItem[],
  logger: Logger,
): Promise<CanonicalWatchObservation[]> {
  const unique = new Map<string, WatchedItem>();
  for (const item of completedWatches(history)) {
    if (!unique.has(item.guid)) unique.set(item.guid, item);
    if (unique.size >= MAX_WATCH_EVIDENCE) break;
  }

  const resolutionQueue = new PQueue({ concurrency: 4 });
  const resolved = await Promise.all(
    [...unique.values()].map((item) =>
      resolutionQueue.add(async () => {
        const resolution = await resolveIdentity(item, logger);
        return resolution.canonicalId &&
          resolution.confidence >= RESOLUTION_CONFIDENCE_THRESHOLD
          ? { canonicalId: resolution.canonicalId, item }
          : undefined;
      }),
    ),
  );

  const detailsQueue = new PQueue({ concurrency: 6 });
  const observations = await Promise.all(
    resolved
      .filter((item): item is { canonicalId: CanonicalId; item: WatchedItem } =>
        Boolean(item),
      )
      .map(({ canonicalId, item }) =>
        detailsQueue.add(async (): Promise<CanonicalWatchObservation> => {
          const tmdbId = Number(canonicalId.split(":")[2]);
          const metadata = await fetchTitleDetails(item.mediaType, tmdbId).catch(
            (error) => {
              logger.warn(
                `Taste metadata lookup failed for ${canonicalId}`,
                (error as Error).message,
              );
              return undefined;
            },
          );
          return { canonicalId, item, metadata };
        }),
      ),
  );
  return observations;
}
