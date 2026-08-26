import type { LivestreamAlertType, LivestreamIntelligenceData } from "./types.js";

export function livestreamAlertConfidenceFloor(input: {
  type: LivestreamAlertType;
  feedbackDigest: string;
  destinySpeakerThreshold: number;
}): number {
  // Destiny presence already passed dedicated speaker, repeated-evidence, and
  // live-conversation gates. Applying the generic semantic-alert floor again
  // can contradict those gates and discard a confirmed participant.
  const relevant = input.feedbackDigest
    .split("\n")
    .filter((line) => line.startsWith(`${input.type}:`));
  const base = input.type === "destiny_guest" ? input.destinySpeakerThreshold : 0.75;
  if (relevant.length < 2) return base;
  const negative = relevant.filter(
    (line) => line.includes("not_useful") || line.includes("false_positive"),
  ).length;
  if (negative / relevant.length < 0.6) return base;
  return input.type === "destiny_guest" ? Math.max(base, 0.75) : 0.9;
}

export function alertSentInSession(
  state: LivestreamIntelligenceData,
  type: LivestreamAlertType,
): boolean {
  const recordedAt = state.alertedAtByType?.[type] ?? 0;
  if (recordedAt >= state.sessionStartedAt) return true;
  return (
    state.latestAlert?.type === type &&
    state.latestAlert.createdAt >= state.sessionStartedAt
  );
}
