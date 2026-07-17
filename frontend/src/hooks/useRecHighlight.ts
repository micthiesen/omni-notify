import { useEffect, useMemo } from "react";

/**
 * Reads the ?recommendation= query param (set by Pushover deep links) and
 * scrolls the matching card into view once the list has loaded.
 */
export function useRecHighlight(loaded: boolean): string | null {
  const highlightedId = useMemo(
    () => new URLSearchParams(window.location.search).get("recommendation"),
    [],
  );

  useEffect(() => {
    if (!highlightedId || !loaded) return;
    document
      .getElementById(`recommendation-${highlightedId}`)
      ?.scrollIntoView({ behavior: "smooth", block: "center" });
  }, [highlightedId, loaded]);

  return highlightedId;
}
