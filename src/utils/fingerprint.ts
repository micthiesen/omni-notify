import { createHash } from "node:crypto";

/**
 * Fingerprint an immutable evidence set: items in stable evidenceId order,
 * keys sorted, undefined fields dropped. Fingerprints are persisted in
 * reflection checkpoints (the no-op guard compares against stored values),
 * so the serialization must stay byte-identical across refactors.
 */
export function fingerprintEvidence<T extends { evidenceId: string }>(
  evidence: T[],
): string {
  const stable = [...evidence]
    .sort((a, b) => a.evidenceId.localeCompare(b.evidenceId))
    .map((item) => JSON.stringify(sortObject(item)))
    .join("\n");
  return digest(stable);
}

function sortObject(value: object): Record<string, unknown> {
  return Object.fromEntries(
    Object.entries(value)
      .filter(([, field]) => field !== undefined)
      .sort(([a], [b]) => a.localeCompare(b)),
  );
}

/** Short stable content hash, also used to derive evidence ids. */
export function digest(value: string): string {
  return createHash("sha256").update(value).digest("hex").slice(0, 24);
}
