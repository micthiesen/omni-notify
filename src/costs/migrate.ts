import { Entity } from "@micthiesen/mitools/entities";
import { hasPrice, TTS_CHARACTER_CENTS } from "../ai/cost.js";
import { BriefingHistoryEntity } from "../briefing-agent/persistence.js";
import { PressPodsEpisodeEntity } from "../press-pods/persistence.js";
import { recordCostEvent } from "./persistence.js";

type CostMigrationData = {
  version: string;
  completedAt: number;
  importedEvents: number;
};

export const CostMigrationEntity = new Entity<CostMigrationData, ["version"]>(
  "cost-migration",
  ["version"],
);

const VERSION = "historical-v1";

/** Seed the ledger once from cost-bearing rows that predate automatic capture. */
export function importHistoricalCosts(): number {
  if (CostMigrationEntity.get({ version: VERSION })) return 0;
  let importedEvents = 0;

  for (const history of BriefingHistoryEntity.getAll()) {
    history.notifications.forEach((notification, index) => {
      if (notification.costCents === undefined) return;
      recordCostEvent({
        eventId: `legacy:briefing:${history.briefingName}:${notification.timestamp}:${index}`,
        incurredAt: notification.timestamp,
        category: "llm",
        feature: "briefings",
        operation: "historical-notification",
        service: "legacy",
        costCents: notification.costCents,
        priceStatus: notification.costCents === null ? "unknown" : "estimated",
        usage: {},
        runId: notification.runId,
      });
      importedEvents++;
    });
  }

  for (const episode of PressPodsEpisodeEntity.getAll()) {
    if (!episode.costs) continue;
    const tokenUsage = Object.values(episode.costs.detailTokens).reduce(
      (total, usage) => ({
        inputTokens: total.inputTokens + usage.input,
        outputTokens: total.outputTokens + usage.output,
      }),
      { inputTokens: 0, outputTokens: 0 },
    );
    const characters = Object.values(episode.costs.detailChars).reduce(
      (sum, value) => sum + value,
      0,
    );
    const llmPricesKnown = Object.keys(episode.costs.detailTokens).every((key) =>
      hasPrice(key.replace(/-(?:meta|clean)$/, "")),
    );
    const ttsPricesKnown = Object.keys(episode.costs.detailChars).every((key) =>
      Object.hasOwn(TTS_CHARACTER_CENTS, key.replace(/-tts$/, "")),
    );
    recordCostEvent({
      eventId: `legacy:press-pods:llm:${episode.episodeId}`,
      incurredAt: episode.createdAt,
      category: "llm",
      feature: "press-pods",
      operation: "historical-episode",
      service: "legacy",
      costCents: llmPricesKnown ? episode.costs.llmCents : null,
      priceStatus: llmPricesKnown ? "estimated" : "unknown",
      usage: tokenUsage,
      runId: episode.runId,
    });
    recordCostEvent({
      eventId: `legacy:press-pods:tts:${episode.episodeId}`,
      incurredAt: episode.createdAt,
      category: "tts",
      feature: "press-pods",
      operation: "historical-episode",
      service: episode.voiceProvider?.toLowerCase() ?? "legacy",
      costCents: ttsPricesKnown ? episode.costs.ttsCents : null,
      priceStatus: !ttsPricesKnown
        ? "unknown"
        : episode.costs.ttsCents === 0
          ? "free"
          : "estimated",
      usage: { characters },
      runId: episode.runId,
    });
    importedEvents += 2;
  }

  CostMigrationEntity.upsert({
    version: VERSION,
    completedAt: Date.now(),
    importedEvents,
  });
  return importedEvents;
}
