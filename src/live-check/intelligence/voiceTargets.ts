import type { Streamer } from "../streamers.js";

export type VoiceTargetObservation = {
  streamer: Pick<Streamer, "id" | "dgg">;
};

/**
 * Pick the DGG streams most worth sampling after the whole live-check tick has
 * finished. Selection cannot happen inside observeLive because platform polls
 * finish out of order, which lets fast DGG feed results permanently occupy the
 * slots before configured streamers finish their network fetches.
 */
export function selectVoiceTargets<T extends VoiceTargetObservation>(
  observations: Iterable<T>,
  limit: number,
): T[] {
  return [...observations]
    .sort(
      (left, right) =>
        (right.streamer.dgg?.viewers ?? 0) - (left.streamer.dgg?.viewers ?? 0) ||
        Number(right.streamer.dgg?.hosted ?? false) -
          Number(left.streamer.dgg?.hosted ?? false) ||
        left.streamer.id.localeCompare(right.streamer.id),
    )
    .slice(0, Math.max(0, limit));
}
