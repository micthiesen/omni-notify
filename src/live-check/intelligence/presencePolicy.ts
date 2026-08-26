import type { DestinyPresence } from "./types.js";
import type { VoiceEvidenceDecision } from "./voiceEvidence.js";

export type VoiceMatchAction =
  | "ignore"
  | "record_possible"
  | "retain_confirmed"
  | "verify";

export function decideVoiceMatchAction(
  evidence: VoiceEvidenceDecision,
  presence: DestinyPresence | undefined,
): VoiceMatchAction {
  if (evidence === "none") return "ignore";
  if (presence?.state === "confirmed") return "retain_confirmed";
  return evidence === "possible" ? "record_possible" : "verify";
}
