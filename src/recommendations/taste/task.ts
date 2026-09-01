import type { Logger } from "@micthiesen/mitools/logging";
import { ScheduledTask } from "@micthiesen/mitools/scheduling";
import { Effect } from "effect";
import { runPromise } from "../../effect/interop.js";
import { getTasteReflectionModel } from "../../ai/registry.js";
import config from "../../utils/config.js";
import { completedWatches } from "../history.js";
import { RESOLUTION_CONFIDENCE_THRESHOLD, resolveIdentity } from "../identity.js";
import { fetchWatchHistoryEffect as fetchWatchHistory } from "../mediaLibrary.js";
import { getAllRecommendations } from "../persistence.js";
import { fetchTitleDetailsEffect as fetchTitleDetails } from "../tmdb/client.js";
import type { CanonicalId, WatchedItem } from "../types.js";
import { runTasteReflection } from "./reflection.js";
import type { CanonicalWatchObservation } from "./types.js";
import {
  effectMessage,
  persistenceEffect,
  RecommendationPersistenceError,
} from "../effect.js";

const MAX_WATCH_EVIDENCE = 160;

export class MediaTasteReflectionTask extends ScheduledTask {
  public readonly name = "TasteReflection";
  public readonly displayName = "Media Taste Reflection";
  public readonly schedule = config.TASTE_REFLECTION_SCHEDULE;
  public override readonly runOnStartup = false;

  private lastRunSummary?: string;
  private readonly logger: Logger;

  public static create(parentLogger: Logger): MediaTasteReflectionTask | null {
    const modelId = config.TASTE_REFLECTION_MODEL ?? "openai:gpt-5.6-luna";
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
    return new MediaTasteReflectionTask(parentLogger);
  }

  private constructor(logger: Logger) {
    super();
    this.logger = logger.extend("TasteReflection");
  }

  public run(): Promise<void> {
    return runPromise(this.runEffect());
  }

  public runEffect() {
    return Effect.gen({ self: this }, function* () {
      const history = yield* fetchWatchHistory();
      if (history.status === "unavailable") {
        this.lastRunSummary = `skipped: ${history.reason}`;
        this.logger.warn(`Taste reflection skipped: ${history.reason}`);
        return;
      }

      const watched = yield* buildCanonicalWatchEvidenceEffect(
        history.value,
        this.logger,
      );
      const { model, modelId } = getTasteReflectionModel();
      const recommendations = yield* persistenceEffect(
        "read recommendations for taste reflection",
        () => getAllRecommendations(),
      );
      const result = yield* runTasteReflection({
        watched,
        recommendations,
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
    });
  }

  public getLastRunSummary(): string | undefined {
    return this.lastRunSummary;
  }
}

export function buildCanonicalWatchEvidenceEffect(
  history: WatchedItem[],
  logger: Logger,
): Effect.Effect<CanonicalWatchObservation[], RecommendationPersistenceError> {
  return Effect.gen(function* () {
    const unique = new Map<string, WatchedItem>();
    for (const item of completedWatches(history)) {
      if (!unique.has(item.guid)) unique.set(item.guid, item);
      if (unique.size >= MAX_WATCH_EVIDENCE) break;
    }

    const resolved = yield* Effect.forEach(
      [...unique.values()],
      (item) =>
        resolveIdentity(item, logger).pipe(
          Effect.map((resolution) =>
            resolution.canonicalId &&
            resolution.confidence >= RESOLUTION_CONFIDENCE_THRESHOLD
              ? { canonicalId: resolution.canonicalId, item }
              : undefined,
          ),
        ),
      { concurrency: 4 },
    );

    const observations = yield* Effect.forEach(
      resolved.filter((item): item is { canonicalId: CanonicalId; item: WatchedItem } =>
        Boolean(item),
      ),
      ({ canonicalId, item }) =>
        Effect.gen(function* () {
          const tmdbId = Number(canonicalId.split(":")[2]);
          const metadata = yield* fetchTitleDetails(item.mediaType, tmdbId).pipe(
            Effect.catch((error) => {
              logger.warn(
                `Taste metadata lookup failed for ${canonicalId}`,
                effectMessage(error),
              );
              return Effect.succeed(undefined);
            }),
          );
          return { canonicalId, item, metadata };
        }),
      { concurrency: 6 },
    );
    return observations;
  });
}
