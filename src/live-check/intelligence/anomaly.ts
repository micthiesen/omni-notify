import type { StreamSession } from "../sessions.js";
import type { Streamer } from "../streamers.js";
import type { SemanticMetadata, ViewerTrend } from "./types.js";

type ViewerSample = {
  at: number;
  viewers: number | null;
  dggViewers: number | null;
};

// Baselines come from samples 5-20 minutes old, so the last few minutes can
// show a jump without diluting the level it is measured against.
const SAMPLE_WINDOW_MS = 20 * 60 * 1000;
const MIN_BASELINE_AGE_MS = 5 * 60 * 1000;
// Audiences keep arriving for a while after go-live; a baseline drawn from
// that ramp makes ordinary growth look like a surge.
const MIN_SESSION_AGE_MS = 20 * 60 * 1000;
const MIN_BASELINE_SAMPLES = 8;
// A baseline whose newer half sits this far above its older half is still
// climbing, so a higher current count is the ramp continuing, not a surge.
const MAX_BASELINE_CLIMB = 0.15;
// Each half needs this many samples before the window can count as flat, so a
// restart or primary switch cannot skip the climb check with a short history.
const MIN_HALF_SAMPLES = 3;
const SURGE_CONFIRMATION_OBSERVATIONS = 2;
const VIEWER_SURGE_PERCENT = 50;
const MIN_VIEWER_SURGE_GAIN = 100;
const DGG_SURGE_PERCENT = 100;
const MIN_DGG_SURGE_GAIN = 30;

const TYPICAL_PEAK_SESSIONS = 10;
const MIN_TYPICAL_PEAK_SESSIONS = 3;
const MIN_TYPICAL_PEAK_SESSION_MS = 30 * 60 * 1000;
const TYPICAL_PEAK_MAX_AGE_MS = 30 * 24 * 60 * 60 * 1000;

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

/**
 * Median summed-viewer peak of the streamer's recent full sessions, or null
 * without enough history. Short restarts are excluded because their peaks
 * understate a normal stream.
 */
export function typicalSessionPeak(
  sessions: readonly StreamSession[],
  now: number,
): number | null {
  const peaks = sessions
    .filter(
      (session) =>
        session.durationMs >= MIN_TYPICAL_PEAK_SESSION_MS &&
        session.endedAt >= now - TYPICAL_PEAK_MAX_AGE_MS,
    )
    .slice(-TYPICAL_PEAK_SESSIONS)
    .map((session) => session.peakViewers);
  return peaks.length >= MIN_TYPICAL_PEAK_SESSIONS ? median(peaks) : null;
}

type BaselineSample = { at: number; value: number };

type Baseline = {
  level: number;
  samples: number;
  shape: "flat" | "climbing" | "sparse";
};

function measureBaseline(
  samples: BaselineSample[],
  windowStart: number,
  windowEnd: number,
): Baseline {
  const midpoint = (windowStart + windowEnd) / 2;
  const older = samples.filter((sample) => sample.at < midpoint);
  const newer = samples.filter((sample) => sample.at >= midpoint);
  const olderLevel = median(older.map((sample) => sample.value));
  const newerLevel = median(newer.map((sample) => sample.value));
  return {
    level: median(samples.map((sample) => sample.value)),
    samples: samples.length,
    shape:
      older.length < MIN_HALF_SAMPLES || newer.length < MIN_HALF_SAMPLES
        ? "sparse"
        : newerLevel > olderLevel * (1 + MAX_BASELINE_CLIMB)
          ? "climbing"
          : "flat",
  };
}

export class ViewerAnomalyTracker {
  private readonly samples = new Map<string, ViewerSample[]>();
  private readonly surgeStreaks = new Map<string, { viewers: number; dgg: number }>();
  private readonly sources = new Map<string, string>();

  observe(input: {
    streamerId: string;
    /** Primary binding's viewers, so a second binding going live is not a jump. */
    viewers: number | null;
    dggViewers: number | null;
    sessionStartedAt: number;
    /** Identifies the primary binding; a change restarts the viewer baseline. */
    sourceKey?: string;
    /** Summed viewers across live bindings, compared against `typicalPeak`. */
    totalViewers?: number | null;
    /** Typical summed session peak; a platform surge must reach it. */
    typicalPeak?: number | null;
    now?: number;
  }): ViewerTrend {
    const now = input.now ?? Date.now();
    if (input.sourceKey !== undefined) {
      const previousSource = this.sources.get(input.streamerId);
      if (previousSource !== undefined && previousSource !== input.sourceKey) {
        // DGG presence does not depend on the primary binding, so keep it.
        const samples = this.samples.get(input.streamerId) ?? [];
        this.samples.set(
          input.streamerId,
          samples.map((sample) => ({ ...sample, viewers: null })),
        );
        const streaks = this.surgeStreaks.get(input.streamerId);
        if (streaks)
          this.surgeStreaks.set(input.streamerId, { ...streaks, viewers: 0 });
      }
      this.sources.set(input.streamerId, input.sourceKey);
    }
    const history = (this.samples.get(input.streamerId) ?? []).filter(
      (sample) => sample.at >= now - SAMPLE_WINDOW_MS,
    );
    const windowStart = now - SAMPLE_WINDOW_MS;
    const windowEnd = now - MIN_BASELINE_AGE_MS;
    const baselineSamples = history.filter((sample) => sample.at <= windowEnd);
    const viewerBaseline = measureBaseline(
      baselineSamples.flatMap((sample) =>
        sample.viewers === null ? [] : [{ at: sample.at, value: sample.viewers }],
      ),
      windowStart,
      windowEnd,
    );
    const dggBaseline = measureBaseline(
      baselineSamples.flatMap((sample) =>
        sample.dggViewers === null ? [] : [{ at: sample.at, value: sample.dggViewers }],
      ),
      windowStart,
      windowEnd,
    );
    const oldest = baselineSamples[0];
    const elapsedMinutes = oldest ? Math.max(1, (now - oldest.at) / 60_000) : 1;
    const oldestViewer = baselineSamples.find((sample) => sample.viewers !== null);
    const viewersPerMinute =
      oldestViewer && input.viewers !== null
        ? (input.viewers - (oldestViewer.viewers ?? 0)) / elapsedMinutes
        : 0;
    const viewerPercent =
      input.viewers === null ? 0 : percentChange(input.viewers, viewerBaseline.level);
    const dggPercent =
      input.dggViewers === null || dggBaseline.level <= 0
        ? null
        : percentChange(input.dggViewers, dggBaseline.level);
    const sessionWarmed = now - input.sessionStartedAt >= MIN_SESSION_AGE_MS;
    const viewerJump =
      sessionWarmed &&
      viewerBaseline.samples >= MIN_BASELINE_SAMPLES &&
      input.viewers !== null &&
      viewerPercent >= VIEWER_SURGE_PERCENT &&
      input.viewers - viewerBaseline.level >= MIN_VIEWER_SURGE_GAIN;
    const typicalPeak = input.typicalPeak ?? null;
    const belowTypicalPeak =
      typicalPeak !== null && (input.totalViewers ?? input.viewers ?? 0) < typicalPeak;
    const viewerSurgeCandidate =
      viewerJump && viewerBaseline.shape === "flat" && !belowTypicalPeak;
    const dggJump =
      sessionWarmed &&
      dggBaseline.samples >= MIN_BASELINE_SAMPLES &&
      dggPercent !== null &&
      dggPercent >= DGG_SURGE_PERCENT &&
      (input.dggViewers ?? 0) - dggBaseline.level >= MIN_DGG_SURGE_GAIN;
    const dggSurgeCandidate = dggJump && dggBaseline.shape === "flat";
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
        `viewers up ${Math.round(viewerPercent)}% (${input.viewers} vs ${Math.round(viewerBaseline.level)} baseline)`,
      );
    }
    if (dggSurge) {
      reasons.push(
        `DGG audience up ${Math.round(dggPercent ?? 0)}% (${input.dggViewers} vs ${Math.round(dggBaseline.level)} baseline)`,
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
      viewerBaseline.samples < MIN_BASELINE_SAMPLES &&
      dggBaseline.samples < MIN_BASELINE_SAMPLES
    ) {
      suppressionReason = `Waiting for ${MIN_BASELINE_SAMPLES} baseline samples`;
    } else if ((viewerSurgeCandidate || dggSurgeCandidate) && !anomalous) {
      suppressionReason = "Confirming the viewer rise with another observation";
    } else if (!anomalous && (viewerJump || dggJump)) {
      const shapes = [
        ...(viewerJump ? [viewerBaseline.shape] : []),
        ...(dggJump ? [dggBaseline.shape] : []),
      ];
      if (shapes.includes("climbing")) {
        suppressionReason = "Audience is still ramping up";
      } else if (shapes.includes("sparse")) {
        suppressionReason = "Waiting for a longer baseline";
      } else if (viewerJump && belowTypicalPeak && typicalPeak !== null) {
        suppressionReason = `Below the typical session peak of ${Math.round(typicalPeak)}`;
      }
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
      baselineViewers: viewerBaseline.samples > 0 ? viewerBaseline.level : null,
      currentDggViewers: input.dggViewers,
      baselineDggViewers: dggBaseline.samples > 0 ? dggBaseline.level : null,
      baselineSamples: Math.max(viewerBaseline.samples, dggBaseline.samples),
      typicalPeakViewers: typicalPeak,
      candidateObservations,
      suppressionReason,
      updatedAt: now,
    };
  }

  clear(streamerId: string): void {
    this.samples.delete(streamerId);
    this.surgeStreaks.delete(streamerId);
    this.sources.delete(streamerId);
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
