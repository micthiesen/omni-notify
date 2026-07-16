/**
 * Three-state result for calls to external/local services, mirroring the
 * live-check LiveStatus.Unknown pattern: an unavailable service must never be
 * interpreted as "empty state" (which would trigger false transitions or
 * decisions made against missing data).
 */
export type FetchResult<T> =
  | { status: "ok"; value: T }
  | { status: "unavailable"; reason: string };

export function unavailable(error: unknown): FetchResult<never> {
  return {
    status: "unavailable",
    reason: error instanceof Error ? error.message : String(error),
  };
}
