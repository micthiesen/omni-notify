export const MAX_CARRIER_CANDIDATES = 3;

export interface CandidateSelection {
  /** Valid candidates in ranked order, deduped, capped at MAX_CARRIER_CANDIDATES. */
  valid: string[];
  /** Candidates dropped because Parcel doesn't recognize them. */
  invalid: string[];
}

/**
 * Splits ranked carrier candidates into valid and invalid codes against the live
 * Parcel carrier list, preserving rank order and dropping duplicates/blanks.
 */
export function selectValidCandidates(
  candidates: string[],
  validCodes: ReadonlySet<string>,
): CandidateSelection {
  const seen = new Set<string>();
  const valid: string[] = [];
  const invalid: string[] = [];
  for (const raw of candidates) {
    const code = raw.trim();
    if (code.length === 0 || seen.has(code)) continue;
    seen.add(code);
    (validCodes.has(code) ? valid : invalid).push(code);
  }
  return { valid: valid.slice(0, MAX_CARRIER_CANDIDATES), invalid };
}
