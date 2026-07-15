import { WATCHED_COMPLETION_THRESHOLD } from "./outcomes.js";
import type { InProgressItem, WatchedItem } from "./types.js";

const DIGEST_LIMIT = 40;

/** Watches that count as completed (ground truth for taste inputs). */
export function completedWatches(watched: WatchedItem[]): WatchedItem[] {
  return watched
    .filter((w) =>
      w.completion === undefined
        ? w.mediaType === "movie" && w.viewCount >= 1
        : w.completion >= WATCHED_COMPLETION_THRESHOLD,
    )
    .sort((a, b) => b.viewedAt - a.viewedAt);
}

/**
 * Compact taste digest injected into model prompts. Derived exclusively from
 * ground-truth watch history — never from recommendation outcome labels.
 */
export function formatHistoryDigest(
  watched: WatchedItem[],
  inProgress: InProgressItem[],
): string {
  const completed = completedWatches(watched).slice(0, DIGEST_LIMIT);
  const lines: string[] = [];

  if (completed.length > 0) {
    lines.push("Recently watched (newest first):");
    for (const item of completed) {
      const year = item.year ? ` (${item.year})` : "";
      const rewatch = item.viewCount > 1 ? ` — rewatched ${item.viewCount}x` : "";
      lines.push(
        `- ${item.title}${year} [${item.mediaType}] — ${formatDate(item.viewedAt)}${rewatch}`,
      );
    }
  } else {
    lines.push("No completed watch history available.");
  }

  if (inProgress.length > 0) {
    lines.push("", "Currently watching:");
    for (const item of inProgress.slice(0, 10)) {
      const year = item.year ? ` (${item.year})` : "";
      lines.push(
        `- ${item.title}${year} [${item.mediaType}] — ${Math.round(item.progress * 100)}% through`,
      );
    }
  }

  return lines.join("\n");
}

function formatDate(epochMs: number): string {
  return new Date(epochMs).toISOString().slice(0, 10);
}
