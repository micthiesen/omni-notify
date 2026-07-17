/**
 * Single loose title normalization for all podcast-recs matching (subscribed
 * shows, iTunes results, RSS episodes, Castro episodes, outcome keys):
 * lowercase, strip diacritics and apostrophes, replace remaining punctuation
 * with spaces, collapse whitespace.
 *
 * Constraint: normalized outputs are only ever compared against other outputs
 * of this same function within a single run — never persisted, never compared
 * across normalizer versions — so the implementation can evolve safely.
 * Apostrophes are removed (not spaced) so "Carroll's" matches "Carrolls";
 * Unicode letters/numbers are kept so non-Latin titles stay distinguishable.
 */
export function normalizeTitle(title: string): string {
  return title
    .normalize("NFKD")
    .replace(/[̀-ͯ]/g, "") // strip diacritics (combining marks)
    .toLowerCase()
    .replace(/['’]/g, "")
    .replace(/[^\p{L}\p{N}]+/gu, " ")
    .trim();
}
