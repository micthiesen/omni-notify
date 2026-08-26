import type { Streamer } from "../streamers.js";
import type { SemanticMetadata, ViewerTrend } from "./types.js";

type ViewerSample = {
  at: number;
  viewers: number | null;
  dggViewers: number | null;
};

const SAMPLE_WINDOW_MS = 30 * 60 * 1000;
const MIN_BASELINE_AGE_MS = 4 * 60 * 1000;
const MIN_SESSION_AGE_MS = 15 * 60 * 1000;
const MIN_BASELINE_SAMPLES = 10;
const SURGE_CONFIRMATION_OBSERVATIONS = 2;

function median(values: number[]): number {
  if (values.length === 0) return 0;
  const sorted = [...values].sort((a, b) => a - b);
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2 === 0
    ? ((sorted[middle - 1] ?? 0) + (sorted[middle] ?? 0)) / 2
    : (sorted[middle] ?? 0);
}

function percentChange(current: number, baseline: number): number {
  if (baseline <= 0) return 0;
  return ((current - baseline) / baseline) * 100;
}

export class ViewerAnomalyTracker {
  private readonly samples = new Map<string, ViewerSample[]>();
  private readonly surgeStreaks = new Map<string, { viewers: number; dgg: number }>();

  observe(input: {
    streamerId: string;
    viewers: number | null;
    dggViewers: number | null;
    sessionStartedAt: number;
    now?: number;
  }): ViewerTrend {
    const now = input.now ?? Date.now();
    const history = (this.samples.get(input.streamerId) ?? []).filter(
      (sample) => sample.at >= now - SAMPLE_WINDOW_MS,
    );
    const baselineSamples = history.filter(
      (sample) => sample.at <= now - MIN_BASELINE_AGE_MS,
    );
    const viewerBaselineValues = baselineSamples
      .map((sample) => sample.viewers)
      .filter((value): value is number => value !== null);
    const viewerBaseline = median(viewerBaselineValues);
    const dggBaseline = median(
      baselineSamples
        .map((sample) => sample.dggViewers)
        .filter((value): value is number => value !== null),
    );
    const oldest = baselineSamples[0];
    const elapsedMinutes = oldest ? Math.max(1, (now - oldest.at) / 60_000) : 1;
    const oldestViewer = baselineSamples.find((sample) => sample.viewers !== null);
    const viewersPerMinute =
      oldestViewer && input.viewers !== null
        ? (input.viewers - (oldestViewer.viewers ?? 0)) / elapsedMinutes
        : 0;
    const viewerPercent =
      input.viewers === null ? 0 : percentChange(input.viewers, viewerBaseline);
    const dggPercent =
      input.dggViewers === null || dggBaseline <= 0
        ? null
        : percentChange(input.dggViewers, dggBaseline);
    const sessionWarmed = now - input.sessionStartedAt >= MIN_SESSION_AGE_MS;
    const viewerSurgeCandidate =
      sessionWarmed &&
      viewerBaselineValues.length >= MIN_BASELINE_SAMPLES &&
      input.viewers !== null &&
      viewerPercent >= 50 &&
      input.viewers - viewerBaseline >= Math.max(100, viewerBaseline * 0.2);
    const dggBaselineValues = baselineSamples
      .map((sample) => sample.dggViewers)
      .filter((value): value is number => value !== null);
    const dggSurgeCandidate =
      sessionWarmed &&
      dggBaselineValues.length >= MIN_BASELINE_SAMPLES &&
      dggPercent !== null &&
      dggPercent >= 100 &&
      (input.dggViewers ?? 0) - dggBaseline >= 30;
    const previousStreaks = this.surgeStreaks.get(input.streamerId) ?? {
      viewers: 0,
      dgg: 0,
    };
    const streaks = {
      viewers: viewerSurgeCandidate ? previousStreaks.viewers + 1 : 0,
      dgg: dggSurgeCandidate ? previousStreaks.dgg + 1 : 0,
    };
    this.surgeStreaks.set(input.streamerId, streaks);
    const viewerSurge = streaks.viewers >= SURGE_CONFIRMATION_OBSERVATIONS;
    const dggSurge = streaks.dgg >= SURGE_CONFIRMATION_OBSERVATIONS;
    const candidateObservations = Math.max(streaks.viewers, streaks.dgg);
    const anomalous = viewerSurge || dggSurge;
    const reasons: string[] = [];
    if (viewerSurge) {
      reasons.push(
        `viewers up ${Math.round(viewerPercent)}% (${input.viewers} vs ${Math.round(viewerBaseline)} baseline)`,
      );
    }
    if (dggSurge) {
      reasons.push(
        `DGG audience up ${Math.round(dggPercent ?? 0)}% (${input.dggViewers} vs ${Math.round(dggBaseline)} baseline)`,
      );
    }
    let suppressionReason: string | null = null;
    if (!sessionWarmed) {
      const minutesRemaining = Math.max(
        1,
        Math.ceil((MIN_SESSION_AGE_MS - (now - input.sessionStartedAt)) / 60_000),
      );
      suppressionReason = `Building a post-start baseline (${minutesRemaining}m remaining)`;
    } else if (
      viewerBaselineValues.length < MIN_BASELINE_SAMPLES &&
      dggBaselineValues.length < MIN_BASELINE_SAMPLES
    ) {
      suppressionReason = `Waiting for ${MIN_BASELINE_SAMPLES} baseline samples`;
    } else if ((viewerSurgeCandidate || dggSurgeCandidate) && !anomalous) {
      suppressionReason = "Confirming the viewer rise with another observation";
    }
    history.push({
      at: now,
      viewers: input.viewers,
      dggViewers: input.dggViewers,
    });
    this.samples.set(input.streamerId, history);
    return {
      percentChange: viewerPercent,
      viewersPerMinute,
      dggPercentChange: dggPercent,
      anomalous,
      reason: reasons.length > 0 ? reasons.join("; ") : null,
      currentViewers: input.viewers,
      baselineViewers: viewerBaselineValues.length > 0 ? viewerBaseline : null,
      currentDggViewers: input.dggViewers,
      baselineDggViewers: dggBaselineValues.length > 0 ? dggBaseline : null,
      baselineSamples: Math.max(viewerBaselineValues.length, dggBaselineValues.length),
      candidateObservations,
      suppressionReason,
      updatedAt: now,
    };
  }

  clear(streamerId: string): void {
    this.samples.delete(streamerId);
    this.surgeStreaks.delete(streamerId);
  }
}

export function computeRelevance(input: {
  streamer: Streamer;
  semantic?: SemanticMetadata;
  trend?: ViewerTrend;
  destinyConfirmed: boolean;
}): { score: number; reasons: string[] } {
  const reasons: string[] = [];
  let score = input.streamer.tier === "primary" ? 40 : 15;
  if (input.streamer.tier === "primary") reasons.push("primary channel");
  if (input.semantic) {
    score += input.semantic.importance * 0.35;
    if (input.semantic.importance >= 65) reasons.push(input.semantic.reason);
  }
  if (input.trend?.anomalous) {
    score += 25;
    if (input.trend.reason) reasons.push(input.trend.reason);
  }
  const dggViewers = input.streamer.dgg?.viewers ?? 0;
  if (dggViewers > 0) {
    score += Math.min(15, Math.log10(dggViewers + 1) * 5);
    if (dggViewers >= 100) reasons.push(`${dggViewers} watching on DGG`);
  }
  if (input.destinyConfirmed) {
    score += 40;
    reasons.push("Destiny detected as a live participant");
  }
  return { score: Math.round(Math.min(100, score)), reasons: reasons.slice(0, 4) };
}
