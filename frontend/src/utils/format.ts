export function pad2(n: number): string {
  return String(n).padStart(2, "0");
}

/**
 * Format a cost given in cents. Sub-cent amounts read as "0.12¢", otherwise
 * dollars ("$0.0034" / "$1.23"). Returns null for null/undefined/non-finite so
 * callers can conditionally render.
 */
export function formatCents(cents: number | null | undefined): string | null {
  if (cents == null || !Number.isFinite(cents)) return null;
  if (cents < 1) return `${cents.toFixed(2)}¢`;
  const dollars = cents / 100;
  return dollars < 1 ? `$${dollars.toFixed(4)}` : `$${dollars.toFixed(2)}`;
}

export function formatClockTime(hour: number, minute: number): string {
  const ampm = hour >= 12 ? "PM" : "AM";
  const h12 = hour % 12 === 0 ? 12 : hour % 12;
  return `${h12}:${pad2(minute)} ${ampm}`;
}

export function formatRelative(epochMs: number, now = Date.now()): string {
  const diff = epochMs - now;
  const future = diff >= 0;
  const absMs = Math.abs(diff);

  if (absMs < 5_000) return future ? "in a moment" : "just now";

  const totalSec = Math.round(absMs / 1000);
  let text: string;
  if (totalSec < 60) {
    text = `${totalSec}s`;
  } else if (totalSec < 3600) {
    text = `${Math.round(totalSec / 60)}m`;
  } else if (totalSec < 86_400) {
    const hours = Math.floor(totalSec / 3600);
    const mins = Math.round((totalSec % 3600) / 60);
    text = mins > 0 ? `${hours}h ${mins}m` : `${hours}h`;
  } else {
    text = `${Math.round(totalSec / 86_400)}d`;
  }

  return future ? `in ${text}` : `${text} ago`;
}

export function formatDuration(ms: number): string {
  if (ms < 1000) return `${Math.max(0, Math.round(ms))}ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)}s`;
  const totalSec = Math.round(ms / 1000);
  const totalMin = Math.floor(totalSec / 60);
  if (totalMin < 60) return `${totalMin}m ${totalSec % 60}s`;
  const totalHours = Math.floor(totalMin / 60);
  const mins = totalMin % 60;
  if (totalHours < 24) return mins > 0 ? `${totalHours}h ${mins}m` : `${totalHours}h`;
  const days = Math.floor(totalHours / 24);
  const hours = totalHours % 24;
  return hours > 0 ? `${days}d ${hours}h` : `${days}d`;
}

/** Ticking countdown: "2h 05m", "3m 12s", "42s", "now". */
export function formatCountdown(ms: number): string {
  if (ms <= 0) return "now";
  const totalSec = Math.floor(ms / 1000);
  const h = Math.floor(totalSec / 3600);
  const m = Math.floor((totalSec % 3600) / 60);
  const s = totalSec % 60;
  if (h > 0) return `${h}h ${pad2(m)}m`;
  if (m > 0) return `${m}m ${pad2(s)}s`;
  return `${s}s`;
}

/** Coarse elapsed time for stream uptime: "1h 23m", "23m", "45s". */
export function formatUptime(ms: number): string {
  const totalSec = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(totalSec / 3600);
  const m = Math.floor((totalSec % 3600) / 60);
  if (h > 0) return `${h}h ${m}m`;
  if (m > 0) return `${m}m`;
  return `${totalSec}s`;
}

export function formatCompactNumber(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 10_000) return `${Math.round(n / 1000)}k`;
  if (n >= 1_000) return `${(n / 1000).toFixed(1)}k`;
  return String(n);
}

export function formatAbsolute(epochMs: number): string {
  return new Date(epochMs).toLocaleString("en-US", {
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
  });
}

export function formatAbsoluteWithYear(epochMs: number): string {
  return new Date(epochMs).toLocaleString("en-US", {
    month: "short",
    day: "numeric",
    year: "numeric",
    hour: "numeric",
    minute: "2-digit",
  });
}

/** "Task runs" → "Task Runs", "CastroInboxCleanup" → "Castro Inbox Cleanup". */
export function toTitleCase(value: string): string {
  return value
    .replace(/[_-]+/g, " ")
    .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
    .replace(/([A-Z]+)([A-Z][a-z])/g, "$1 $2")
    .trim()
    .replace(/\s+/g, " ")
    .split(" ")
    .map((word) => (/^[a-z]/.test(word) ? word[0].toUpperCase() + word.slice(1) : word))
    .join(" ");
}

/** Prefers a task's admin-set displayName, falling back to a title-cased name. */
export function taskLabel(task: { name: string; displayName?: string | null }): string {
  return task.displayName?.trim() || toTitleCase(task.name);
}

/**
 * Looks up a task by name (e.g. from a TaskRun's taskName) and returns its
 * label; falls back to a title-cased name when the task isn't found (run-only
 * contexts where the task list isn't available or the task was removed).
 */
export function taskLabelFromName(
  taskName: string,
  tasks: { name: string; displayName?: string | null }[],
): string {
  const task = tasks.find((t) => t.name === taskName);
  return task ? taskLabel(task) : toTitleCase(taskName);
}

export function formatDateOnly(epochMs: number): string {
  return new Date(epochMs).toLocaleDateString("en-US", {
    month: "short",
    day: "numeric",
    year: "numeric",
  });
}

/** Formats a YYYY-MM-DD value without allowing timezone conversion to shift it. */
export function formatCalendarDate(date: string, includeYear = true): string {
  const [year, month, day] = date.split("-").map(Number);
  if (!year || !month || !day) return date;
  return new Date(year, month - 1, day).toLocaleDateString("en-US", {
    month: "short",
    day: "numeric",
    ...(includeYear ? { year: "numeric" } : {}),
  });
}
