import cronParser from "cron-parser";

const HOUR_MS = 60 * 60 * 1000;
const MIN_CATCH_UP_CADENCE_MS = 6 * HOUR_MS;
const MIN_CATCH_UP_WINDOW_MS = 6 * HOUR_MS;
const MAX_CATCH_UP_WINDOW_MS = 48 * HOUR_MS;
const OCCURRENCES_PER_SIDE = 4;

export type CatchUpDecision =
  | { kind: "none" }
  | { kind: "disabled"; cadenceMs: number }
  | {
      kind: "stale";
      scheduledFor: number;
      latenessMs: number;
      maxLatenessMs: number;
    }
  | {
      kind: "run";
      scheduledFor: number;
      latenessMs: number;
      maxLatenessMs: number;
    };

/**
 * Decide whether the newest cron occurrence since the persisted cursor should
 * run now. At most one occurrence is recovered, regardless of backlog size.
 */
export function decideCatchUp(
  schedule: string,
  evaluatedThrough: number,
  now: number,
): CatchUpDecision {
  const occurrences = getNearbyOccurrences(schedule, now);
  const previous = occurrences.filter((time) => time <= now).at(-1);
  if (previous === undefined || previous <= evaluatedThrough) return { kind: "none" };

  const cadenceMs = getMinimumGap(occurrences);
  if (cadenceMs <= MIN_CATCH_UP_CADENCE_MS) {
    return { kind: "disabled", cadenceMs };
  }

  const maxLatenessMs = Math.min(
    MAX_CATCH_UP_WINDOW_MS,
    Math.max(MIN_CATCH_UP_WINDOW_MS, cadenceMs / 4),
  );
  const latenessMs = now - previous;
  if (latenessMs > maxLatenessMs) {
    return { kind: "stale", scheduledFor: previous, latenessMs, maxLatenessMs };
  }
  return { kind: "run", scheduledFor: previous, latenessMs, maxLatenessMs };
}

function getNearbyOccurrences(schedule: string, now: number): number[] {
  const previous = cronParser.parseExpression(schedule, { currentDate: new Date(now) });
  const next = cronParser.parseExpression(schedule, { currentDate: new Date(now) });
  const occurrences: number[] = [];

  for (let i = 0; i < OCCURRENCES_PER_SIDE; i++) {
    occurrences.push(previous.prev().getTime());
    occurrences.push(next.next().getTime());
  }
  return occurrences.sort((a, b) => a - b);
}

function getMinimumGap(occurrences: number[]): number {
  let minimum = Number.POSITIVE_INFINITY;
  for (let i = 1; i < occurrences.length; i++) {
    minimum = Math.min(minimum, occurrences[i] - occurrences[i - 1]);
  }
  return minimum;
}
